//! Native seams for Cozy-managed project processes: explicit descriptor inheritance, basic
//! process core-dump suppression, and owned-process observation. This is NOT an OS filesystem,
//! VM, network, or arbitrary-custom-Python sandbox. OS-owned core/swap policy is outside it.
#![allow(unsafe_code)]
use crate::Result;
use std::{fs::File, process::Command};

/// Native descriptors deliberately handed to a managed child. The caller validates every
/// ProjectWritable file against its actual project root before constructing this manifest.
pub enum PrivateDescriptor {
    ReadOnly(File),
    ProjectWritable(File),
    KernelIpc(File),
}

/// Set a trusted parent's core limit to zero before it handles secrets. The compiled parent
/// never restores it. This does not constrain an external host administrator or OS snapshots.
pub fn disable_project_parent_dumps() -> Result<()> {
    imp::parent_cores()
}
/// Basic child/parent defense only; intentionally no piped-handler or filesystem ABI gate.
pub fn disable_private_core_dumps() -> Result<()> {
    disable_project_parent_dumps()
}

/// Prepare an ordinary managed process. No sandbox claim: only explicitly listed native
/// descriptors survive exec, and Cozy does not request process cores. Working directory and
/// project HOME/cache/temp/stdio are set by the caller's owned-storage adapter.
pub fn prepare_managed_child(
    command: &mut Command,
    inherited: Vec<PrivateDescriptor>,
) -> Result<()> {
    imp::prepare(command, inherited)
}

/// Native authority check before a trusted parent exports a descriptor. Read-only source
/// authority comes from its existing object/grant resolver; writable disk files must be in the
/// actual project. Deleted disk files are not treated as anonymous RAM.
pub fn validate_project_descriptor(root: &File, file: &File) -> Result<()> {
    imp::validate(root, file)
}
/// A managed descendant still alive (including a stuck kernel process) prevents a seal receipt.
pub fn project_scope_quiescent(scope: &crate::contain::Scope) -> Result<bool> {
    imp::quiescent(scope)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::{
        fs, io,
        os::{
            fd::AsRawFd,
            unix::{fs::MetadataExt, process::CommandExt},
        },
        path::{Path, PathBuf},
    };
    fn denied(message: &'static str) -> crate::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message).into()
    }
    pub(super) fn parent_cores() -> Result<()> {
        // SAFETY: native resource structure and scalar arguments are valid for these syscalls.
        unsafe {
            if libc::setrlimit(
                libc::RLIMIT_CORE,
                &libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                },
            ) < 0
            {
                return Err(io::Error::last_os_error().into());
            }
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) < 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
    fn mode(file: &File) -> Result<i32> {
        // SAFETY: F_GETFL reads flags of this live owned descriptor.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(flags & libc::O_ACCMODE)
    }
    #[cfg(target_os = "linux")]
    fn path(file: &File) -> Result<PathBuf> {
        Ok(fs::read_link(format!(
            "/proc/self/fd/{}",
            file.as_raw_fd()
        ))?)
    }
    #[cfg(not(target_os = "linux"))]
    fn path(_: &File) -> Result<PathBuf> {
        Err(crate::Error::Unsupported {
            seam: crate::Seam::PrivateExecution,
            os: crate::Os::CURRENT,
        })
    }
    fn anonymous_ram(file: &File) -> Result<bool> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: statfs is a plain native output structure, initialized by fstatfs.
            let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
            // SAFETY: descriptor and exact output pointer are valid for the call.
            if unsafe { libc::fstatfs(file.as_raw_fd(), &mut stat) } < 0 {
                return Err(io::Error::last_os_error().into());
            }
            Ok(file.metadata()?.nlink() == 0 && stat.f_type as u64 == 0x01021994)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = file;
            Ok(false)
        }
    }
    fn ipc(file: &File) -> Result<()> {
        let kind = file.metadata()?.mode() & libc::S_IFMT;
        if kind == libc::S_IFIFO {
            return Ok(());
        }
        if kind == libc::S_IFREG && anonymous_ram(file)? {
            return Ok(());
        }
        if kind != libc::S_IFSOCK {
            return Err(denied("managed IPC manifest contains a filesystem object"));
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: output structures are live and their sizes are supplied to getsockopt.
            unsafe {
                let mut domain = 0i32;
                let mut kind = 0i32;
                let mut peer = std::mem::zeroed::<libc::ucred>();
                let mut size = std::mem::size_of::<i32>() as libc::socklen_t;
                let mut peer_size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                if libc::getsockopt(
                    file.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_DOMAIN,
                    (&mut domain as *mut i32).cast(),
                    &mut size,
                ) < 0
                    || libc::getsockopt(
                        file.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_TYPE,
                        (&mut kind as *mut i32).cast(),
                        &mut size,
                    ) < 0
                    || libc::getsockopt(
                        file.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        (&mut peer as *mut libc::ucred).cast(),
                        &mut peer_size,
                    ) < 0
                {
                    return Err(io::Error::last_os_error().into());
                }
                if domain != libc::AF_UNIX
                    || kind != libc::SOCK_STREAM
                    || peer.uid != libc::geteuid()
                {
                    return Err(denied("managed IPC is not an owned local stream"));
                }
            }
        }
        Ok(())
    }
    pub(super) fn validate(root: &File, file: &File) -> Result<()> {
        let stat = file.metadata()?;
        if stat.is_file() && mode(file)? == libc::O_RDONLY {
            return Ok(());
        }
        if stat.is_file() && anonymous_ram(file)? {
            return Ok(());
        }
        if stat.is_file() || stat.is_dir() {
            let root = path(root)?;
            let actual = path(file)?;
            if !root.is_absolute()
                || !Path::new(&actual).starts_with(&root)
                || actual
                    .as_os_str()
                    .as_encoded_bytes()
                    .ends_with(b" (deleted)")
            {
                return Err(denied("managed writable descriptor is outside its project"));
            }
            let found = fs::metadata(actual)?;
            if found.dev() != stat.dev() || found.ino() != stat.ino() {
                return Err(denied("managed descriptor changed filesystem identity"));
            }
            return Ok(());
        }
        ipc(file)
    }
    pub(super) fn prepare(command: &mut Command, inherited: Vec<PrivateDescriptor>) -> Result<()> {
        let mut files = vec![];
        for descriptor in inherited {
            let file = match descriptor {
                PrivateDescriptor::ReadOnly(file) => {
                    if !file.metadata()?.is_file() || mode(&file)? != libc::O_RDONLY {
                        return Err(denied(
                            "managed read-only manifest has the wrong native access or type",
                        ));
                    }
                    file
                }
                PrivateDescriptor::ProjectWritable(file) => {
                    if !file.metadata()?.is_file() && !file.metadata()?.is_dir() {
                        return Err(denied(
                            "managed writable manifest has the wrong native type",
                        ));
                    }
                    file
                }
                PrivateDescriptor::KernelIpc(file) => {
                    ipc(&file)?;
                    file
                }
            };
            if file.as_raw_fd() < 3 {
                return Err(denied("managed inherited descriptors must not alias stdio"));
            }
            files.push(file);
        }
        crate::process::inherit_nothing(command);
        // SAFETY: callback uses only async-signal-safe native syscalls over owned live handles.
        unsafe {
            command.pre_exec(move || {
                if libc::setrlimit(
                    libc::RLIMIT_CORE,
                    &libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    },
                ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                for file in &files {
                    if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(())
    }
    pub(super) fn quiescent(scope: &crate::contain::Scope) -> Result<bool> {
        #[cfg(target_os = "linux")]
        {
            match scope.processes() {
                Ok(count) => Ok(count == 0),
                Err(error) => {
                    let error: io::Error = error.into();
                    if error.kind() == io::ErrorKind::NotFound {
                        Ok(true)
                    } else {
                        Err(error.into())
                    }
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = scope;
            Err(crate::Error::Unsupported {
                seam: crate::Seam::PrivateExecution,
                os: crate::Os::CURRENT,
            })
        }
    }
}
#[cfg(not(unix))]
mod imp {
    use super::*;
    fn unsupported() -> crate::Error {
        crate::Error::Unsupported {
            seam: crate::Seam::PrivateExecution,
            os: crate::Os::CURRENT,
        }
    }
    pub(super) fn parent_cores() -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn validate(_: &File, _: &File) -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn prepare(_: &mut Command, _: Vec<PrivateDescriptor>) -> Result<()> {
        Err(unsupported())
    }
    pub(super) fn quiescent(_: &crate::contain::Scope) -> Result<bool> {
        Err(unsupported())
    }
}
