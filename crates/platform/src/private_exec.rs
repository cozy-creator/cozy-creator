//! Native project filesystem policy. Callers provide reviewed native handles, never wire paths.
//! Linux Landlock controls subsequent opens and descendants; seccomp closes descriptor/core escapes.
//! This is filesystem custody, not a VM or network isolation boundary.
#![allow(unsafe_code)]

use crate::Result;
use std::{fs::File, process::Command};

/// Every filesystem resource the child may read or modify.
pub struct PrivateResources {
    pub project_root: File,
    pub read_only_trees: Vec<File>,
    pub read_only_files: Vec<File>,
    pub executables: Vec<File>,
    pub devices: Vec<File>,
    pub inherited: Vec<PrivateDescriptor>,
}

/// Descriptors deliberately retained across exec. Writable regular files must be in the project.
pub enum PrivateDescriptor {
    ReadOnly(File),
    ProjectWritable(File),
    KernelIpc(File),
}

/// A prepared policy owns the ruleset and reviewed descriptors until its last command exits.
#[derive(Clone)]
pub struct PrivateWritePolicy(std::sync::Arc<imp::Policy>);

pub fn prepare_project_writes(resources: PrivateResources) -> Result<PrivateWritePolicy> {
    imp::prepare(resources).map(|policy| PrivateWritePolicy(std::sync::Arc::new(policy)))
}

impl PrivateWritePolicy {
    /// Installs the policy before the first executable sees private bytes. Any install failure
    /// fails spawn. Other pre_exec callbacks must not add descriptors or privileges after this.
    pub fn apply(&self, command: &mut Command) -> Result<()> {
        imp::apply(self.0.clone(), command)
    }
    /// Enter a delegated kernel cgroup before the policy and first exec. Token-only
    /// process scopes cannot prove arbitrary descendant shutdown and are unsupported.
    pub fn apply_in_scope(
        &self,
        command: &mut Command,
        scope: &crate::contain::Scope,
    ) -> Result<()> {
        imp::enter_scope(command, scope)?;
        self.apply(command)
    }
}

/// Lingering descendants, including a kernel-stuck process, prevent a seal receipt.
pub fn project_scope_quiescent(scope: &crate::contain::Scope) -> Result<bool> {
    imp::scope_quiescent(scope)
}

/// Validate an ancillary descriptor before the trusted worker sends it to a confined child.
/// Read-only source authority is established by the caller's existing object/grant resolver.
/// Writable filesystem descriptors must belong to the project; anonymous writable memory must
/// be kernel RAM-backed and unlinked, never an arbitrary deleted disk file.
pub fn validate_project_descriptor(project_root: &File, descriptor: &File) -> Result<()> {
    imp::validate_descriptor(project_root, descriptor)
}

/// Suppress dumps in a trusted parent before handling secrets. This parent must never restore
/// the settings or exec untrusted code. It does not constrain an external host administrator.
pub fn disable_project_parent_dumps() -> Result<()> {
    imp::parent_cores()
}

/// Trusted parents call this before reading their first private record, and never restore it.
pub fn disable_private_core_dumps() -> Result<()> {
    imp::disable_cores()
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
mod imp {
    use super::*;
    pub(super) struct Policy;
    fn unsupported() -> crate::Error {
        crate::Error::Unsupported {
            seam: crate::Seam::PrivateExecution,
            os: crate::Os::CURRENT,
        }
    }
    pub(super) fn prepare(_: PrivateResources) -> Result<Policy> {
        Err(unsupported())
    }
    pub(super) fn apply(_: std::sync::Arc<Policy>, _: &mut Command) -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn enter_scope(_: &mut Command, _: &crate::contain::Scope) -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn scope_quiescent(_: &crate::contain::Scope) -> Result<bool> {
        Err(unsupported())
    }
    pub(super) fn validate_descriptor(_: &File, _: &File) -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn parent_cores() -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn disable_cores() -> Result<()> {
        Err(unsupported())
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod imp {
    use super::*;
    use std::{
        fs, io,
        os::{
            fd::{AsRawFd, FromRawFd, RawFd},
            unix::{fs::MetadataExt, process::CommandExt},
        },
        path::{Path, PathBuf},
        sync::Arc,
    };
    const EXECUTE: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const READ: u64 = 1 << 2;
    const READ_DIR: u64 = 1 << 3;
    // ABI3's complete filesystem surface. No rights are silently dropped on older kernels.
    const ALL: u64 = (1 << 15) - 1;
    // Device creation is always denied: a writable device node could bypass filesystem custody.
    const MUTATE: u64 = ALL & !(EXECUTE | READ | READ_DIR | (1 << 6) | (1 << 11));
    #[repr(C)]
    struct Ruleset {
        handled_access_fs: u64,
    }
    #[repr(C, packed)]
    struct Beneath {
        allowed_access: u64,
        parent_fd: i32,
    }

    pub(super) struct Policy {
        ruleset: File,
        _resources: PrivateResources,
        kept: Vec<RawFd>,
        filter: Vec<libc::sock_filter>,
        root: PathBuf,
        read_paths: Vec<(PathBuf, bool)>,
    }

    fn invalid(detail: &'static str) -> crate::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, detail).into()
    }
    fn syscall_result(result: libc::c_long) -> Result<libc::c_long> {
        if result < 0 {
            Err(io::Error::last_os_error().into())
        } else {
            Ok(result)
        }
    }
    fn core_pattern() -> Result<()> {
        let pattern = fs::read_to_string("/proc/sys/kernel/core_pattern")?;
        if pattern.trim_start().starts_with('|') {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "protected project execution refuses a piped kernel core handler",
            )
            .into());
        }
        Ok(())
    }
    pub(super) fn parent_cores() -> Result<()> {
        // SAFETY: prctl/setrlimit receive valid scalar arguments and a live native structure.
        unsafe {
            syscall_result(libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) as _)?;
            syscall_result(libc::setrlimit(
                libc::RLIMIT_CORE,
                &libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                },
            ) as _)?;
        }
        Ok(())
    }
    pub(super) fn disable_cores() -> Result<()> {
        parent_cores()?;
        core_pattern()
    }
    fn mode(file: &File) -> Result<i32> {
        // SAFETY: F_GETFL reads only this owned, live descriptor's flags.
        Ok(
            syscall_result(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) } as _)? as i32
                & libc::O_ACCMODE,
        )
    }
    fn path(file: &File) -> Result<PathBuf> {
        let path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        if !path.is_absolute() || path.to_string_lossy().ends_with(" (deleted)") {
            return Err(invalid(
                "private resource has no stable filesystem identity",
            ));
        }
        Ok(path)
    }
    fn within(file: &File, root: &Path) -> Result<()> {
        let actual = path(file)?;
        if !actual.starts_with(root) {
            return Err(invalid(
                "writable inherited descriptor is outside the project",
            ));
        }
        let stat = file.metadata()?;
        let resolved = fs::metadata(&actual)?;
        if stat.dev() != resolved.dev() || stat.ino() != resolved.ino() {
            return Err(invalid("private resource changed identity"));
        }
        Ok(())
    }
    fn read_only(file: &File, directory: bool) -> Result<()> {
        let stat = file.metadata()?;
        if (directory && !stat.is_dir())
            || (!directory && !stat.is_file())
            || mode(file)? != libc::O_RDONLY
        {
            return Err(invalid(
                "read-only resource has the wrong native type or access mode",
            ));
        }
        Ok(())
    }
    fn validate_local_parent(file: &File) -> Result<()> {
        let mut domain: i32 = 0;
        let mut kind: i32 = 0;
        // SAFETY: every bit pattern is valid for this plain native output structure.
        let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
        let mut int_size = std::mem::size_of::<i32>() as libc::socklen_t;
        let mut peer_size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt receives live native output structures and their exact sizes.
        unsafe {
            syscall_result(libc::getsockopt(
                file.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_DOMAIN,
                (&mut domain as *mut i32).cast(),
                &mut int_size,
            ) as _)?;
            syscall_result(libc::getsockopt(
                file.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut i32).cast(),
                &mut int_size,
            ) as _)?;
            syscall_result(libc::getsockopt(
                file.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut peer_size,
            ) as _)?;
            if domain != libc::AF_UNIX
                || kind != libc::SOCK_STREAM
                || peer.pid != libc::getpid()
                || peer.uid != libc::geteuid()
            {
                return Err(invalid(
                    "IPC channel is not a stream created by this trusted parent",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn validate_descriptor(project: &File, file: &File) -> Result<()> {
        let stat = file.metadata()?;
        let kind = stat.mode() & libc::S_IFMT;
        match kind {
            libc::S_IFREG if mode(file)? == libc::O_RDONLY => Ok(()),
            libc::S_IFREG => {
                if within(file, &path(project)?).is_ok() {
                    return Ok(());
                }
                // SAFETY: fstatfs writes only the exact native output structure.
                let mut filesystem = unsafe { std::mem::zeroed::<libc::statfs>() };
                // SAFETY: the descriptor and output pointer are live for fstatfs.
                syscall_result(unsafe { libc::fstatfs(file.as_raw_fd(), &mut filesystem) } as _)?;
                if stat.nlink() == 0 && filesystem.f_type as u64 == 0x01021994 {
                    Ok(())
                } else {
                    Err(invalid(
                        "writable ancillary file is neither project-owned nor anonymous RAM",
                    ))
                }
            }
            libc::S_IFSOCK => validate_local_parent(file),
            libc::S_IFIFO => Ok(()),
            _ => Err(invalid(
                "ancillary descriptor has no approved project resource type",
            )),
        }
    }
    fn rule(ruleset: &File, file: &File, access: u64) -> Result<()> {
        let attr = Beneath {
            allowed_access: access,
            parent_fd: file.as_raw_fd(),
        };
        // SAFETY: Landlock receives the kernel-defined packed attribute and live descriptors.
        syscall_result(unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                1,
                &attr,
                0,
            )
        })?;
        Ok(())
    }
    fn reject_external_links(root: &Path) -> Result<()> {
        let mut pending = vec![root.to_owned()];
        let mut files = std::collections::BTreeMap::<(u64, u64), (u64, u64)>::new();
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let stat = fs::symlink_metadata(entry.path())?;
                if matches!(stat.mode() & libc::S_IFMT, libc::S_IFCHR | libc::S_IFBLK) {
                    return Err(invalid("project contains an unreviewed device node"));
                }
                if stat.is_dir() {
                    pending.push(entry.path());
                } else if stat.is_file() && stat.nlink() > 1 {
                    let counted = files
                        .entry((stat.dev(), stat.ino()))
                        .or_insert((stat.nlink(), 0));
                    counted.1 += 1;
                }
            }
        }
        if files.values().any(|(links, seen)| links != seen) {
            return Err(invalid(
                "project contains a regular file hard-linked outside its owned tree",
            ));
        }
        Ok(())
    }
    pub(super) fn prepare(resources: PrivateResources) -> Result<Policy> {
        core_pattern()?;
        let root = path(&resources.project_root)?;
        let stat = resources.project_root.metadata()?;
        // SAFETY: geteuid has no preconditions.
        if !stat.is_dir()
            || stat.uid() != unsafe { libc::geteuid() }
            || root == Path::new("/")
            || std::env::home_dir().as_ref() == Some(&root)
        {
            return Err(invalid("project root is not an owned native directory"));
        }
        reject_external_links(&root)?;
        // SAFETY: the ABI query takes a null attribute, zero size and VERSION flag.
        let abi = syscall_result(unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<Ruleset>(),
                0,
                1,
            )
        })?;
        if abi < 3 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "protected project execution requires Landlock ABI 3",
            )
            .into());
        }
        let attr = Ruleset {
            handled_access_fs: ALL,
        };
        // SAFETY: native ruleset structure is live for the syscall; returned fd is uniquely owned.
        let fd = syscall_result(unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr,
                std::mem::size_of::<Ruleset>(),
                0,
            )
        })? as RawFd;
        // SAFETY: successful create_ruleset returned a new descriptor.
        let ruleset = unsafe { File::from_raw_fd(fd) };
        rule(&ruleset, &resources.project_root, MUTATE | READ | READ_DIR)?;
        for file in &resources.read_only_trees {
            read_only(file, true)?;
            let read_root = path(file)?;
            if read_root == Path::new("/")
                || root.starts_with(&read_root)
                || std::env::home_dir().as_ref() == Some(&read_root)
            {
                return Err(invalid(
                    "read-only tree is an overbroad project or credential ancestor",
                ));
            }
            rule(&ruleset, file, READ | READ_DIR)?;
        }
        for file in &resources.read_only_files {
            read_only(file, false)?;
            rule(&ruleset, file, READ)?;
        }
        for file in &resources.executables {
            read_only(file, false)?;
            rule(&ruleset, file, READ | EXECUTE)?;
        }
        for file in &resources.devices {
            if file.metadata()?.mode() & libc::S_IFMT != libc::S_IFCHR {
                return Err(invalid("device exception is not a native character device"));
            }
            rule(&ruleset, file, READ | WRITE)?;
        }
        let mut kept = vec![];
        for descriptor in &resources.inherited {
            let file = match descriptor {
                PrivateDescriptor::ReadOnly(file) => {
                    read_only(file, false)?;
                    file
                }
                PrivateDescriptor::ProjectWritable(file) => {
                    let stat = file.metadata()?;
                    if !stat.is_file() && !stat.is_dir() {
                        return Err(invalid(
                            "project-writable descriptor has the wrong native type",
                        ));
                    }
                    within(file, &root)?;
                    file
                }
                PrivateDescriptor::KernelIpc(file) => {
                    let kind = file.metadata()?.mode() & libc::S_IFMT;
                    if !matches!(kind, libc::S_IFSOCK | libc::S_IFIFO) {
                        return Err(invalid("kernel IPC descriptor is a filesystem escape"));
                    }
                    if kind == libc::S_IFSOCK {
                        validate_local_parent(file)?;
                    }
                    file
                }
            };
            kept.push(file.as_raw_fd());
        }
        let read_paths = resources
            .read_only_trees
            .iter()
            .map(|file| Ok((path(file)?, true)))
            .chain(
                resources
                    .read_only_files
                    .iter()
                    .chain(resources.executables.iter())
                    .map(|file| Ok((path(file)?, false))),
            )
            .collect::<Result<Vec<_>>>()?;
        let protected: Vec<_> = resources
            .inherited
            .iter()
            .filter_map(|resource| match resource {
                PrivateDescriptor::KernelIpc(file) => Some(file.as_raw_fd()),
                _ => None,
            })
            .collect();
        if protected.len() > 64 || protected.iter().any(|fd| *fd < 3) {
            return Err(invalid(
                "too many protected IPC descriptors or invalid descriptor number",
            ));
        }
        Ok(Policy {
            ruleset,
            _resources: resources,
            kept,
            filter: filter(&protected),
            root,
            read_paths,
        })
    }

    fn statement(code: u16, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }
    fn jump(k: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter {
            code: 0x15,
            jt,
            jf,
            k,
        }
    }
    fn filter(protected: &[RawFd]) -> Vec<libc::sock_filter> {
        const ALLOW: u32 = 0x7fff0000;
        const DENY: u32 = 0x00050000 | libc::EPERM as u32;
        #[cfg(target_arch = "x86_64")]
        const ARCH: u32 = 0xc000003e;
        #[cfg(target_arch = "aarch64")]
        const ARCH: u32 = 0xc00000b7;
        let mut code = vec![
            statement(0x20, 4),
            jump(ARCH, 1, 0),
            statement(0x06, 0x80000000),
            statement(0x20, 0),
        ];
        // Refuse the alternate x32 syscall ABI too, even with the x86_64 audit architecture.
        #[cfg(target_arch = "x86_64")]
        code.extend([
            libc::sock_filter {
                code: 0x45,
                jt: 0,
                jf: 1,
                k: 0x40000000,
            },
            statement(0x06, DENY),
        ]);
        // A trusted parent-created IPC fd is pinned: no close/reuse, rebind, CLOEXEC or
        // dup2/dup3 target replacement can turn its recvmsg allowance into another socket.
        let width = (protected.len() * 2 + 2) as u8;
        code.extend([
            jump(libc::SYS_recvmsg as u32, 0, width),
            statement(0x20, 16),
        ]);
        for fd in protected {
            code.extend([jump(*fd as u32, 0, 1), statement(0x06, ALLOW)]);
        }
        code.extend([statement(0x06, DENY), statement(0x20, 0)]);
        let mut pinned_calls = vec![
            (libc::SYS_close, 16),
            (libc::SYS_connect, 16),
            (libc::SYS_dup3, 24),
        ];
        #[cfg(target_arch = "x86_64")]
        pinned_calls.push((libc::SYS_dup2, 24));
        for (syscall, argument) in pinned_calls {
            code.extend([jump(syscall as u32, 0, width), statement(0x20, argument)]);
            for fd in protected {
                code.extend([jump(*fd as u32, 0, 1), statement(0x06, DENY)]);
            }
            code.extend([statement(0x06, ALLOW), statement(0x20, 0)]);
        }
        // Whitelist descriptor/query/locking fcntls. Inode write-hint and other metadata
        // commands are not allowed through a read-only public file descriptor.
        let safe_fcntl = [
            libc::F_DUPFD,
            libc::F_DUPFD_CLOEXEC,
            libc::F_GETFD,
            libc::F_GETFL,
            libc::F_SETFL,
            libc::F_GETLK,
            libc::F_SETLK,
            libc::F_SETLKW,
            libc::F_OFD_GETLK,
            libc::F_OFD_SETLK,
            libc::F_OFD_SETLKW,
            libc::F_GET_SEALS,
            libc::F_GETPIPE_SZ,
            libc::F_SETPIPE_SZ,
        ];
        let mut fcntl = vec![
            statement(0x20, 24),
            jump(libc::F_SETFD as u32, (safe_fcntl.len() * 2 + 1) as u8, 0),
        ];
        for cmd in safe_fcntl {
            fcntl.extend([jump(cmd as u32, 0, 1), statement(0x06, ALLOW)]);
        }
        fcntl.extend([statement(0x06, DENY), statement(0x20, 16)]);
        for fd in protected {
            fcntl.extend([jump(*fd as u32, 0, 1), statement(0x06, DENY)]);
        }
        fcntl.push(statement(0x06, ALLOW));
        code.push(jump(libc::SYS_fcntl as u32, 0, fcntl.len() as u8));
        code.extend(fcntl);
        code.push(statement(0x20, 0));
        // clone3 hides namespace flags behind a pointer. ENOSYS permits libc's ordinary
        // clone fallback; namespace-creating clone flags remain denied explicitly.
        code.extend([
            jump(libc::SYS_clone3 as u32, 0, 1),
            statement(0x06, 0x00050000 | libc::ENOSYS as u32),
        ]);
        code.extend([
            jump(libc::SYS_clone as u32, 0, 4),
            statement(0x20, 16),
            libc::sock_filter {
                code: 0x45,
                jt: 1,
                jf: 0,
                k: 0x7e020000,
            },
            statement(0x06, ALLOW),
            statement(0x06, DENY),
            statement(0x20, 0),
        ]);
        // Never acquire another process's descriptors/address space, a filesystem bypass, or
        // an unfiltered asynchronous I/O submission path. recvmsg can import writable SCM_RIGHTS.
        for syscall in [
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_pidfd_getfd,
            libc::SYS_open_by_handle_at,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_setns,
            libc::SYS_unshare,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_io_uring_setup,
            libc::SYS_recvmmsg,
            libc::SYS_ioctl,
            libc::SYS_close_range,
            libc::SYS_getxattr,
            libc::SYS_lgetxattr,
            libc::SYS_fgetxattr,
            // Landlock ABI3 does not mediate inode metadata mutation. Deny these paths too;
            // callers requiring them must fail, rather than gain writes outside the project.
            libc::SYS_fchmod,
            libc::SYS_fchmodat,
            libc::SYS_fchmodat2,
            libc::SYS_fchown,
            libc::SYS_fchownat,
            libc::SYS_utimensat,
            libc::SYS_setxattr,
            libc::SYS_lsetxattr,
            libc::SYS_fsetxattr,
            libc::SYS_removexattr,
            libc::SYS_lremovexattr,
            libc::SYS_fremovexattr,
        ] {
            code.extend([jump(syscall as u32, 0, 1), statement(0x06, DENY)]);
        }
        #[cfg(target_arch = "x86_64")]
        for syscall in [
            libc::SYS_chmod,
            libc::SYS_chown,
            libc::SYS_lchown,
            libc::SYS_utime,
            libc::SYS_utimes,
            libc::SYS_futimesat,
        ] {
            code.extend([jump(syscall as u32, 0, 1), statement(0x06, DENY)]);
        }
        // Blocking all setrlimit is deliberate: a package cannot restore RLIMIT_CORE after exec.
        code.extend([
            jump(libc::SYS_setrlimit as u32, 0, 1),
            statement(0x06, DENY),
        ]);
        // prlimit64(pid, resource, new_limit, old_limit): read-only queries remain valid.
        code.extend([
            jump(libc::SYS_prlimit64 as u32, 0, 6),
            statement(0x20, 32),
            jump(0, 0, 3),
            statement(0x20, 36),
            jump(0, 0, 1),
            statement(0x06, ALLOW),
            statement(0x06, DENY),
            statement(0x20, 0),
        ]);
        // prctl(PR_SET_DUMPABLE, nonzero) is refused; no other prctl can restore core limits.
        code.extend([
            jump(libc::SYS_prctl as u32, 0, 7),
            statement(0x20, 16),
            jump(libc::PR_SET_DUMPABLE as u32, 0, 5),
            statement(0x20, 24),
            jump(0, 0, 1),
            statement(0x06, ALLOW),
            statement(0x06, DENY),
            statement(0x20, 0),
        ]);
        code.push(statement(0x06, ALLOW));
        code
    }

    fn scope_path(scope: &crate::contain::Scope) -> Result<PathBuf> {
        let relative = scope.cgroup_relative().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "protected project execution requires a delegated kernel cgroup",
            )
        })?;
        let relative = relative
            .strip_prefix('/')
            .ok_or_else(|| invalid("invalid native process scope"))?;
        if Path::new(relative)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(invalid("invalid native process scope"));
        }
        Ok(Path::new("/sys/fs/cgroup").join(relative))
    }
    pub(super) fn enter_scope(command: &mut Command, scope: &crate::contain::Scope) -> Result<()> {
        let file = File::options()
            .write(true)
            .open(scope_path(scope)?.join("cgroup.procs"))?;
        // SAFETY: callback only writes the kernel's self-PID token to an owned live cgroup fd.
        unsafe {
            command.pre_exec(move || {
                if libc::write(file.as_raw_fd(), b"0".as_ptr().cast(), 1) != 1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
    pub(super) fn scope_quiescent(scope: &crate::contain::Scope) -> Result<bool> {
        match fs::read_to_string(scope_path(scope)?.join("cgroup.events")) {
            Ok(events) => Ok(events.lines().any(|line| line == "populated 0")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn apply(policy: Arc<Policy>, command: &mut Command) -> Result<()> {
        core_pattern()?;
        // Keep the caller's reviewed project subdirectory (uv lock/build relies on it).
        // A missing cwd defaults to the owned root; an outside cwd is never accepted.
        let cwd = command
            .get_current_dir()
            .map(fs::canonicalize)
            .transpose()?
            .unwrap_or_else(|| policy.root.clone());
        if !cwd.starts_with(&policy.root) {
            return Err(invalid("command working directory is outside its project"));
        }
        command.current_dir(cwd);
        // SAFETY: callback runs only async-signal-safe syscalls, with owned immutable structures.
        unsafe {
            command.pre_exec(move || {
                // Recheck the actual cwd after Command chdir and earlier callbacks, before exec.
                let mut cwd = [0u8; 4096];
                let count = libc::readlink(
                    c"/proc/self/cwd".as_ptr(),
                    cwd.as_mut_ptr().cast(),
                    cwd.len(),
                );
                if count <= 0 || count as usize == cwd.len() {
                    return Err(io::Error::from_raw_os_error(libc::EPERM));
                }
                let cwd = std::ffi::OsStr::from_encoded_bytes_unchecked(&cwd[..count as usize]);
                if !Path::new(cwd).starts_with(&policy.root)
                    || cwd.as_encoded_bytes().ends_with(b" (deleted)")
                {
                    return Err(io::Error::from_raw_os_error(libc::EPERM));
                }
                // stdio was installed by Command before pre_exec. It must not be a writable file
                // outside the project, even if another caller configured it incorrectly.
                for fd in 0..=2 {
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    if flags < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    {
                        let mut stat = std::mem::zeroed::<libc::stat>();
                        if libc::fstat(fd, &mut stat) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                        if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
                            // The actual stdio file was opened by the trusted parent. Verify its
                            // current native identity against the owned project before spawning.
                            let target =
                                [c"/proc/self/fd/0", c"/proc/self/fd/1", c"/proc/self/fd/2"]
                                    [fd as usize];
                            let mut bytes = [0u8; 4096];
                            let count = libc::readlink(
                                target.as_ptr(),
                                bytes.as_mut_ptr().cast(),
                                bytes.len(),
                            );
                            if count < 0 || count as usize == bytes.len() {
                                return Err(io::Error::last_os_error());
                            }
                            let path = std::ffi::OsStr::from_encoded_bytes_unchecked(
                                &bytes[..count as usize],
                            );
                            let path = Path::new(path);
                            let readable = flags & libc::O_ACCMODE == libc::O_RDONLY
                                && policy.read_paths.iter().any(|(allowed, tree)| {
                                    if *tree {
                                        path.starts_with(allowed)
                                    } else {
                                        path == allowed
                                    }
                                });
                            if !path.starts_with(&policy.root) && !readable {
                                return Err(io::Error::from_raw_os_error(libc::EPERM));
                            }
                        } else {
                            let approved = policy
                                ._resources
                                .devices
                                .iter()
                                .chain(policy._resources.inherited.iter().map(|d| match d {
                                    PrivateDescriptor::ReadOnly(f)
                                    | PrivateDescriptor::ProjectWritable(f)
                                    | PrivateDescriptor::KernelIpc(f) => f,
                                }))
                                .any(|f| {
                                    let mut allowed = std::mem::zeroed::<libc::stat>();
                                    libc::fstat(f.as_raw_fd(), &mut allowed) == 0
                                        && stat.st_dev == allowed.st_dev
                                        && stat.st_ino == allowed.st_ino
                                });
                            if !approved {
                                return Err(io::Error::from_raw_os_error(libc::EPERM));
                            }
                        }
                    }
                }
                // CLOEXEC every unreviewed descriptor, including earlier keep_open callbacks.
                // close_range leaves Rust's exec-error pipe alive until exec, preserving spawn errors.
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) < 0 {
                    return Err(io::Error::last_os_error());
                }
                for target in [3, 4] {
                    let mut actual = std::mem::zeroed::<libc::stat>();
                    if libc::fstat(target, &mut actual) == 0
                        && policy._resources.inherited.iter().any(|resource| {
                            let PrivateDescriptor::KernelIpc(file) = resource else {
                                return false;
                            };
                            let mut expected = std::mem::zeroed::<libc::stat>();
                            libc::fstat(file.as_raw_fd(), &mut expected) == 0
                                && actual.st_dev == expected.st_dev
                                && actual.st_ino == expected.st_ino
                        })
                        && libc::fcntl(target, libc::F_SETFD, 0) < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                for fd in &policy.kept {
                    if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
                    || libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) < 0
                    || libc::setrlimit(
                        libc::RLIMIT_CORE,
                        &libc::rlimit {
                            rlim_cur: 0,
                            rlim_max: 0,
                        },
                    ) < 0
                    || libc::syscall(
                        libc::SYS_landlock_restrict_self,
                        policy.ruleset.as_raw_fd(),
                        0,
                    ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                // No retained root capabilities may authorize a later filesystem/device escape.
                #[repr(C)]
                struct CapabilityHeader {
                    version: u32,
                    pid: i32,
                }
                #[repr(C)]
                struct CapabilityData {
                    effective: u32,
                    permitted: u32,
                    inheritable: u32,
                }
                let header = CapabilityHeader {
                    version: 0x20080522,
                    pid: 0,
                };
                let data = [
                    CapabilityData {
                        effective: 0,
                        permitted: 0,
                        inheritable: 0,
                    },
                    CapabilityData {
                        effective: 0,
                        permitted: 0,
                        inheritable: 0,
                    },
                ];
                if libc::syscall(libc::SYS_capset, &header, data.as_ptr()) < 0
                    || libc::prctl(
                        libc::PR_CAP_AMBIENT,
                        libc::PR_CAP_AMBIENT_CLEAR_ALL,
                        0,
                        0,
                        0,
                    ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                let program = libc::sock_fprog {
                    len: policy.filter.len() as u16,
                    filter: policy.filter.as_ptr().cast_mut(),
                };
                if libc::prctl(libc::PR_SET_SECCOMP, 2, &program, 0, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}
