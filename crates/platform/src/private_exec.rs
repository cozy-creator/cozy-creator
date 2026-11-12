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
    const MUTATE: u64 = ALL & !(EXECUTE | READ | READ_DIR);
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
    pub(super) fn disable_cores() -> Result<()> {
        core_pattern()?;
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
                    within(file, &root)?;
                    file
                }
                PrivateDescriptor::KernelIpc(file) => {
                    let kind = file.metadata()?.mode() & libc::S_IFMT;
                    if !matches!(kind, libc::S_IFSOCK | libc::S_IFIFO) {
                        return Err(invalid("kernel IPC descriptor is a filesystem escape"));
                    }
                    file
                }
            };
            kept.push(file.as_raw_fd());
        }
        Ok(Policy {
            ruleset,
            _resources: resources,
            kept,
            filter: filter(),
            root,
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
    fn filter() -> Vec<libc::sock_filter> {
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
            libc::SYS_recvmsg,
            libc::SYS_recvmmsg,
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
                    if flags & libc::O_ACCMODE != libc::O_RDONLY {
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
                            if count < 0 {
                                return Err(io::Error::last_os_error());
                            }
                            let path = std::ffi::OsStr::from_encoded_bytes_unchecked(
                                &bytes[..count as usize],
                            );
                            if !Path::new(path).starts_with(&policy.root) {
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
