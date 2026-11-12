//! What Linux and macOS share: process groups, sessions, signals and readiness.
#![allow(unsafe_code)]
use rustix::{
    event::{PollFd, PollFlags, Timespec},
    io::Errno,
    process::{Pid, Resource, Signal, getrlimit, kill_process_group, setrlimit},
};
use std::{
    ffi::OsString,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, RawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use super::HANDED;

/// Marks every descriptor from `first` up close-on-exec in this process. In a child about to exec
/// it leaves the new program only what its spawner hands on afterwards; at a process's start it
/// keeps whatever it inherited from anything it starts. Async-signal-safe: `close_range` (Linux
/// 5.11), else each descriptor to the soft limit.
pub fn cloexec_from(first: u32) {
    // SAFETY: plain syscalls on this process's own descriptor table.
    unsafe {
        #[cfg(target_os = "linux")]
        if libc::syscall(
            libc::SYS_close_range,
            first,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        ) == 0
        {
            return;
        }
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let last = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            limit.rlim_cur.min(1 << 20) as u32
        } else {
            1 << 16
        };
        for fd in first..last {
            libc::fcntl(fd as i32, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// [`super::inherit_nothing`] on Unix.
pub(super) fn inherit_nothing(command: &mut Command) {
    // SAFETY: cloexec_from is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            cloexec_from(3);
            Ok(())
        });
    }
}

/// The group is the leader's PID: nothing to hold.
#[derive(Debug)]
pub(super) struct Group;

pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Group)> {
    command.process_group(0);
    Ok((command.spawn()?, Group))
}

#[derive(Debug)]
pub(super) struct Detached(Child);

impl Detached {
    pub(super) fn pid(&self) -> u32 {
        self.0.id()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait()
    }
}

/// A session of its own: no terminal's hangup reaches it, and it leads its own group.
pub(super) fn detach(program: &Path, args: &[OsString], log: Option<File>) -> io::Result<Detached> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    match log {
        Some(log) => command.stdout(log.try_clone()?).stderr(log),
        None => command.stdout(Stdio::null()).stderr(Stdio::null()),
    };
    // SAFETY: cloexec_from and setsid are async-signal-safe and touch no memory.
    unsafe {
        command.pre_exec(|| {
            cloexec_from(3);
            rustix::process::setsid()?;
            Ok(())
        });
    }
    command.spawn().map(Detached)
}

pub(super) fn kill_group(_: &Group, leader: &Child) -> io::Result<()> {
    signal_group(leader.id())
}

pub(super) fn signal_group(group: u32) -> io::Result<()> {
    let leader = Pid::from_raw(group as i32).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    signaled(kill_process_group(leader, Signal::KILL), group)
}

/// A signal's result, where a target already gone is done. macOS answers EPERM, not ESRCH, for
/// a process (or a group led by one) that has exited and is not yet reaped.
pub(super) fn signaled(result: rustix::io::Result<()>, pid: u32) -> io::Result<()> {
    match result {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(Errno::PERM) if super::sys::exited(pid) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Whether `fd` turned readable within `period` (`None`: wait for it).
pub(super) fn readable(fd: BorrowedFd<'_>, period: Option<Duration>) -> io::Result<bool> {
    let timeout = period.map(|period| Timespec::try_from(period).map_err(io::Error::other)).transpose()?;
    let mut poll = [PollFd::new(&fd, PollFlags::IN)];
    loop {
        match rustix::event::poll(&mut poll, timeout.as_ref()) {
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(0) => return Ok(false),
            Ok(_) if !poll[0].revents().contains(PollFlags::IN) => {
                return Err(io::Error::other("process exit is not observable"));
            }
            Ok(_) => return Ok(true),
        }
    }
}

pub(super) fn raise_fd_limit() {
    let mut limit = getrlimit(Resource::Nofile);
    // macOS refuses a soft limit above OPEN_MAX (10240) even under an unlimited hard limit.
    let ceiling = if cfg!(target_vendor = "apple") {
        Some(limit.maximum.map_or(10240, |maximum| maximum.min(10240)))
    } else {
        limit.maximum
    };
    if limit.current.is_some_and(|current| ceiling.is_none_or(|ceiling| current < ceiling)) {
        limit.current = ceiling;
        let _ = setrlimit(Resource::Nofile, limit);
    }
}

pub(super) fn fd_limit() -> Option<u64> {
    getrlimit(Resource::Nofile).current
}

/// [`super::keep_open`] on Unix.
pub(super) fn keep_open(command: &mut Command, fds: Vec<RawFd>) {
    // SAFETY: only async-signal-safe fcntl, in the child; a descriptor not open fails the spawn.
    unsafe {
        command.pre_exec(move || {
            for &fd in &fds {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// Whether [`reserve_handed`] ran: until it has, nothing may be placed on a handed descriptor.
static RESERVED: AtomicBool = AtomicBool::new(false);

/// [`super::reserve_handed`].
pub(super) fn reserve_handed() -> io::Result<()> {
    // One thread: nothing can open a file between a slot's check and its fill.
    if threads()? != 1 {
        return Err(io::Error::other("this process has more than one thread"));
    }
    for fd in HANDED {
        // SAFETY: a query of a descriptor number; nothing is read or written.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0 {
            continue;
        }
        // `fd` is closed, so the lowest free descriptor a fresh open takes is at most `fd`.
        if let Ok(null) = File::open("/dev/null") {
            // SAFETY: `fd` is closed (checked above), so no owner loses its descriptor; `null`
            // is ours and stays open through the call.
            if null.as_raw_fd() != fd && unsafe { libc::dup2(null.as_raw_fd(), fd) } >= 0 {
                // SAFETY: as above, on the copy dup2 just made.
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            } else if null.as_raw_fd() == fd {
                std::mem::forget(null);
            }
        }
    }
    RESERVED.store(true, Ordering::Release);
    Ok(())
}

/// [`super::take_handed`].
pub(super) fn take_handed(fd: RawFd) -> Option<File> {
    // SAFETY: duplicates `fd` if it is open; the copy is new and ours, `fd` stays as it was.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    // SAFETY: a descriptor fcntl just created for this process alone.
    (copy >= 0).then(|| unsafe { File::from_raw_fd(copy) })
}

/// [`super::hand_on_exec`].
pub(super) fn hand_on_exec(fd: &impl AsRawFd, at: RawFd) -> io::Result<()> {
    if !HANDED.contains(&at) || !RESERVED.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a reserved handed descriptor"));
    }
    // SAFETY: `at` is a reserved slot, which no owner holds (reserve_handed); dup2 replaces
    // whatever unowned descriptor sits there and leaves the copy inheritable.
    if unsafe { libc::dup2(fd.as_raw_fd(), at) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if fd.as_raw_fd() == at {
        // SAFETY: as above; dup2 onto itself keeps close-on-exec, so clear it.
        if unsafe { libc::fcntl(at, libc::F_SETFD, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// [`super::fork`].
pub(super) fn fork() -> io::Result<super::Forked> {
    if threads()? != 1 {
        return Err(io::Error::other("this process has more than one thread"));
    }
    // SAFETY: this process has one thread (checked above, and only this thread could start
    // another), so the child inherits no lock another thread holds.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(super::Forked::Child),
        pid => Ok(super::Forked::Parent(pid as u32)),
    }
}

#[cfg(target_os = "linux")]
fn threads() -> io::Result<usize> {
    Ok(std::fs::read_dir("/proc/self/task")?.count())
}

#[cfg(target_os = "macos")]
fn threads() -> io::Result<usize> {
    // SAFETY: plain-old-data the call fills.
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: a writable buffer of exactly `size` bytes.
    let filled = unsafe {
        libc::proc_pidinfo(std::process::id() as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&mut task as *mut libc::proc_taskinfo).cast(), size)
    };
    if filled != size {
        return Err(io::Error::last_os_error());
    }
    Ok(task.pti_threadnum as usize)
}

/// What wakes a supervising parent: TERM, INT and CHLD, blocked on its thread and taken one at a
/// time.
pub struct SupervisorSignals(libc::sigset_t);

/// What [`SupervisorSignals::wait`] took.
pub enum Woken {
    Stop(Signal),
    Child,
}

/// A `pthread_sigmask` or `sigwait` answer: the error itself, not -1 and errno.
fn answered(code: libc::c_int) -> io::Result<()> {
    match code {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(code)),
    }
}

impl SupervisorSignals {
    /// TERM, INT and CHLD, not yet blocked.
    pub fn new() -> Self {
        // SAFETY: plain-old-data the calls fill, and valid signal numbers.
        unsafe {
            let mut set = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
                libc::sigaddset(&mut set, signal);
            }
            Self(set)
        }
    }

    /// Blocks them on this thread, so only [`Self::wait`] takes them.
    pub fn block(&self) -> io::Result<()> {
        // SAFETY: a set this value holds; the old mask is not asked for.
        answered(unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &self.0, std::ptr::null_mut()) })
    }

    pub fn wait(&self) -> io::Result<Woken> {
        let mut signal = 0;
        // SAFETY: a set this value holds and a writable signal number.
        answered(unsafe { libc::sigwait(&self.0, &mut signal) })?;
        Ok(match signal {
            libc::SIGTERM => Woken::Stop(Signal::TERM),
            libc::SIGINT => Woken::Stop(Signal::INT),
            _ => Woken::Child,
        })
    }
}

impl Default for SupervisorSignals {
    fn default() -> Self {
        Self::new()
    }
}

/// This thread blocks no signal: what a forked service starts from.
pub fn unblock_signals() -> io::Result<()> {
    // SAFETY: plain-old-data the call fills, then a set on this stack; the old mask is not asked for.
    answered(unsafe {
        let mut none = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut())
    })
}

/// [`super::keep_sigchld`].
pub(super) fn keep_sigchld() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        extern "C" fn noted(_: libc::c_int) {}
        // SAFETY: sigaction is plain old data; all-zero is a valid value.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = noted as extern "C" fn(libc::c_int) as usize;
        // SAFETY: a handler that does nothing is async-signal-safe; `action` outlives the call.
        if unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// [`super::glibc_version`].
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn glibc_version() -> String {
    // SAFETY: glibc returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    #[test]
    fn a_test_harness_has_threads_so_fork_and_reservation_are_refused() {
        let more = std::thread::spawn(|| std::thread::park_timeout(std::time::Duration::from_secs(5)));
        assert!(super::fork().is_err());
        assert!(super::reserve_handed().is_err());
        more.thread().unpark();
        more.join().unwrap();
    }

    #[test]
    fn a_handed_descriptor_is_copied_and_an_unreserved_slot_refused() {
        let (mut read, mut write) = std::io::pipe().unwrap();
        write.write_all(b"handed").unwrap();
        drop(write);
        let mut copy = super::take_handed(std::os::fd::AsRawFd::as_raw_fd(&read)).unwrap();
        let mut text = String::new();
        copy.read_to_string(&mut text).unwrap();
        assert_eq!(text, "handed");
        assert_eq!(read.read(&mut [0u8; 1]).unwrap(), 0, "the original stays open, at its end");
        assert!(super::take_handed(-1).is_none());
        assert!(super::hand_on_exec(&read, 7).is_err());
    }
}

/// Two private inherited descriptors for Chromium's debugging protocol. Both child endpoints
/// are first moved above fd 4, so installing 3/4 cannot clobber either source.
#[cfg(target_os = "linux")]
pub(super) fn private_browser_pipe(command: &mut Command) -> io::Result<(File, File)> {
    fn pipe() -> io::Result<(File, File)> {
        let mut fds = [-1; 2];
        // SAFETY: pipe fills the two slots; each successful descriptor gets exactly one File owner.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 { return Err(io::Error::last_os_error()); }
        let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        for file in [&read, &write] {
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 { return Err(io::Error::last_os_error()); }
        }
        Ok((read, write))
    }
    fn above(file: File) -> io::Result<File> {
        // SAFETY: fcntl duplicates this owned descriptor; the returned descriptor gets one owner.
        let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
        if fd == -1 { return Err(io::Error::last_os_error()); }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    let (child_read, parent_write) = pipe()?;
    let (parent_read, child_write) = pipe()?;
    let (child_read, child_write) = (above(child_read)?, above(child_write)?);
    inherit_nothing(command);
    let parent = std::process::id();
    // SAFETY: only async-signal-safe descriptor/process syscalls runs in the child. Captured File owners keep the exact
    // endpoints alive until spawn; dup2 clears CLOEXEC on 3/4, other descriptors retain it.
    unsafe {
        command.pre_exec(move || {
            // A controller crash cannot leave its Chrome leader running with live authority.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 || libc::getppid() as u32 != parent {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "private browser parent ended"));
            }
            if libc::dup2(child_read.as_raw_fd(), 3) == -1 || libc::dup2(child_write.as_raw_fd(), 4) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((parent_write, parent_read))
}
