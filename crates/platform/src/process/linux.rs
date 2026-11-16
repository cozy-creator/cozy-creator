//! Linux: births from `/proc`, exact handles by pidfd (TensorD `process.rs`).
#![allow(unsafe_code)]
use super::{Birth, State, unix};
use rustix::{
    io::Errno,
    process::{
        Pid, PidfdFlags, Signal, getpid, getppid, pidfd_open, pidfd_send_signal, set_child_subreaper,
        set_parent_process_death_signal, setpriority_process,
    },
};
use std::{
    fs, io,
    os::fd::{AsFd, OwnedFd},
    time::Duration,
};

#[derive(Debug)]
pub(super) struct Handle(pub(super) OwnedFd);

pub(super) fn birth(pid: u32) -> io::Result<Birth> {
    Ok(Birth {
        pid,
        boot_id: boot_id()?,
        start_ticks: stat(pid)?.start_ticks,
    })
}

pub(super) fn state(birth: &Birth) -> io::Result<State> {
    const PF_EXITING: u64 = 0x4;
    if boot_id()? != birth.boot_id {
        return Ok(State::Gone);
    }
    let stat = match stat(birth.pid) {
        Ok(stat) => stat,
        Err(error) if gone(&error) => return Ok(State::Gone),
        Err(error) => return Err(error),
    };
    if stat.start_ticks != birth.start_ticks || stat.exited() {
        return Ok(State::Gone);
    }
    if stat.flags & PF_EXITING != 0 || sigkill_pending(birth.pid)? {
        return Ok(State::Dying);
    }
    Ok(match stat.state {
        'T' | 't' => State::Stopped,
        _ => State::Live,
    })
}

pub(super) fn open(birth: &Birth) -> io::Result<Option<Handle>> {
    if boot_id()? != birth.boot_id {
        return Ok(None);
    }
    let pid = Pid::from_raw(birth.pid as i32).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(pidfd) => pidfd,
        // ESRCH: reaped. EINVAL: a leader already exiting (its threads tearing down).
        Err(Errno::SRCH | Errno::INVAL) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // Read after opening: the pidfd now pins whichever task holds the PID.
    match stat(birth.pid) {
        Ok(stat) if stat.start_ticks == birth.start_ticks => Ok(Some(Handle(pidfd))),
        Ok(_) => Ok(None),
        Err(error) if gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn kill(handle: &Handle, birth: &Birth) -> io::Result<()> {
    // A group's number is not reused while a member lives, so a group this birth leads is its own.
    let leads_group = stat(birth.pid)
        .is_ok_and(|stat| stat.start_ticks == birth.start_ticks && stat.pgrp as u32 == birth.pid);
    unix::signaled(pidfd_send_signal(&handle.0, Signal::KILL), birth.pid)?;
    if leads_group {
        unix::signal_group(birth.pid)?;
    }
    Ok(())
}

pub(super) fn wait(handle: &Handle, period: Option<Duration>) -> io::Result<bool> {
    unix::readable(handle.0.as_fd(), period)
}

pub(super) fn background_thread() {
    // All act on the calling thread alone: Linux keeps nice and the I/O class per thread.
    let _ = setpriority_process(Some(rustix::thread::gettid()), 19);
    // SAFETY: ioprio_set takes scalars; IOPRIO_WHO_PROCESS 0 is the caller. rustix has no ioprio_set.
    unsafe { libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13) };
}

pub(super) fn background_process() {
    let _ = setpriority_process(Some(rustix::thread::gettid()), 10);
    // SAFETY: as in `background_thread`; the best-effort class (2) at its lowest level (7).
    unsafe { libc::syscall(libc::SYS_ioprio_set, 1, 0, (2 << 13) | 7) };
}

pub(super) fn set_name(name: &str) -> crate::Result<()> {
    let name = std::ffi::CString::new(name)
        .map_err(|error| crate::Error::os("name this process")(io::Error::new(io::ErrorKind::InvalidInput, error)))?;
    // The kernel copies at most 16 bytes.
    rustix::thread::set_name(&name).map_err(|error| crate::Error::os("name this process")(error.into()))
}

pub(super) fn parent_of(pid: u32) -> io::Result<u32> {
    Ok(stat(pid)?.ppid)
}

/// Every process with its parent.
pub(super) fn parents() -> Vec<(u32, u32)> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| Some((pid, stat(pid).ok()?.ppid)))
        .collect()
}

pub(super) fn group_members(leader: &Birth) -> Vec<Birth> {
    let Ok(boot) = boot_id() else {
        return vec![];
    };
    if boot != leader.boot_id {
        return vec![];
    }
    let Ok(entries) = fs::read_dir("/proc") else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != leader.pid)
        .filter_map(|pid| {
            let stat = stat(pid).ok()?;
            (stat.pgrp as u32 == leader.pid && stat.start_ticks >= leader.start_ticks).then(|| {
                Birth {
                    pid,
                    boot_id: boot.clone(),
                    start_ticks: stat.start_ticks,
                }
            })
        })
        .collect()
}

/// Owed is the time its live threads waited runnable for a CPU (`schedstat` run delay): busy
/// cores or a container's spent CPU quota slow a working process, never still it.
pub(super) fn meter(pid: u32) -> Option<(u64, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    let ticks = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    let hertz = rustix::param::clock_ticks_per_second().max(1);
    let waited: u64 = fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|task| fs::read_to_string(task.path().join("schedstat")).ok())
        .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        .sum();
    let cpu = ticks
        .saturating_mul(1_000_000_000 / hertz)
        .saturating_add(waited);
    let moved = fs::read_to_string(format!("/proc/{pid}/io")).map_or(0, |io| io_bytes(&io));
    Some((cpu, moved))
}

pub(super) fn lives(pid: u32) -> Option<Vec<super::Life>> {
    let hertz = rustix::param::clock_ticks_per_second().max(1);
    let mut lives = vec![];
    for task in fs::read_dir(format!("/proc/{pid}/task")).ok()?.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|tid| tid.parse().ok()) else { continue };
        let base = task.path();
        // A thread gone mid-read is left out.
        let Ok(stat) = fs::read_to_string(base.join("stat")) else { continue };
        let Some((head, rest)) = stat.rsplit_once(") ") else { continue };
        if rest.starts_with(['Z', 'X']) {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let ticks = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
        // Run time, run delay and runs, in nanoseconds: a wakeup between ticks shows. Ticks where the kernel keeps no schedstat.
        let scheduled: Vec<u64> = fs::read_to_string(base.join("schedstat")).unwrap_or_default()
            .split_whitespace().filter_map(|n| n.parse::<u64>().ok()).collect();
        let meter = super::Meter {
            cpu_ns: match scheduled.as_slice() {
                [ran, waited, ..] => ran.saturating_add(*waited),
                _ => ticks.saturating_mul(1_000_000_000 / hertz),
            },
            runs: scheduled.get(2).copied().unwrap_or(0),
            bytes: fs::read_to_string(base.join("io")).map_or(0, |io| io_bytes(&io)),
            faults: fields.get(9).and_then(|faults| faults.parse().ok()).unwrap_or(0),
        };
        let wait = match fs::read_to_string(base.join("syscall")) {
            Ok(line) => wait_of(pid, &line),
            Err(error) if gone(&error) => continue,
            Err(_) => super::Wait::Unknown,
        };
        let name = head.split_once(" (").map_or("", |(_, name)| name).to_owned();
        lives.push(super::Life { tid, name, meter, wait });
    }
    Some(lives)
}

fn io_bytes(io: &str) -> u64 {
    io.lines()
        .filter_map(|line| line.split_once(": "))
        .filter(|(name, _)| matches!(*name, "rchar" | "wchar" | "read_bytes" | "write_bytes"))
        .filter_map(|(_, value)| value.trim().parse::<u64>().ok())
        .sum()
}

/// What a blocked syscall (`nr a1 … a6 sp pc`) waits on. Per architecture: x86_64's and aarch64's numbers.
fn wait_of(pid: u32, line: &str) -> super::Wait {
    use super::Wait;
    let mut fields = line.split_whitespace();
    // `running`, or `-1 …`: in the kernel outside a syscall (a fault).
    let Some(nr) = fields.next().and_then(|nr| nr.parse::<i64>().ok()).filter(|nr| *nr >= 0) else { return Wait::Running };
    let arg: Vec<u64> = fields.take(6).filter_map(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok()).collect();
    let at = |i: usize| arg.get(i).copied().unwrap_or(0);
    let given = |i: usize| at(i) != 0;
    let millis = |i: usize| at(i) as i32 >= 0;
    let on = |fds: Vec<i32>| match crate::net::tcp_waits(pid, &fds) {
        peers if peers.is_empty() => Wait::Still,
        peers => Wait::Peers { peers },
    };
    let fd = |i: usize| on(vec![at(i) as i32]);
    let polled = |timed: bool| if timed { Wait::Time } else { on(poll_fds(pid, at(0), at(1))) };
    let selected = |timed: bool| if timed { Wait::Time } else { on(select_fds(pid, at(0), at(1))) };
    let epolled = |timed: bool| if timed { Wait::Time } else { on(epoll_fds(pid, at(0) as i32)) };
    // FUTEX_WAIT, LOCK_PI, WAIT_BITSET, WAIT_REQUEUE_PI, LOCK_PI2, past the private and clock flags.
    let futex = || if matches!(at(1) & 0x7f, 0 | 6 | 9 | 11 | 13) && given(3) { Wait::Time } else { Wait::Still };
    let timed = |yes: bool| if yes { Wait::Time } else { Wait::Still };
    #[cfg(target_arch = "x86_64")]
    return match nr {
        35 | 230 => Wait::Time,
        202 => futex(),
        449 => timed(given(3)),
        0 | 17 | 19 | 45 | 47 => fd(0),
        299 => if given(4) { Wait::Time } else { fd(0) },
        7 => polled(millis(2)),
        271 => polled(given(2)),
        23 | 270 => selected(given(4)),
        232 | 281 => epolled(millis(3)),
        441 => epolled(given(3)),
        _ => Wait::Still,
    };
    #[cfg(target_arch = "aarch64")]
    return match nr {
        101 | 115 => Wait::Time,
        98 => futex(),
        449 => timed(given(3)),
        63 | 65 | 67 | 207 | 212 => fd(0),
        243 => if given(4) { Wait::Time } else { fd(0) },
        73 => polled(given(2)),
        72 => selected(given(4)),
        22 => epolled(millis(3)),
        441 => epolled(given(3)),
        _ => Wait::Still,
    };
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (nr, on, fd, polled, selected, epolled, futex, timed, millis);
        Wait::Unknown
    }
}

/// `count` bytes of the process's memory at `address`, as its ancestor may read them.
fn memory(pid: u32, address: u64, count: usize) -> Option<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    let mut bytes = vec![0; count];
    fs::File::open(format!("/proc/{pid}/mem")).ok()?.read_exact_at(&mut bytes, address).ok()?;
    Some(bytes)
}

/// The descriptors of a `poll` array (`struct pollfd`: an int and two shorts).
fn poll_fds(pid: u32, address: u64, count: u64) -> Vec<i32> {
    memory(pid, address, 8 * count.min(1024) as usize).map_or_else(Vec::new, |bytes| {
        bytes.as_chunks::<8>().0.iter().map(|entry| i32::from_ne_bytes([entry[0], entry[1], entry[2], entry[3]])).filter(|fd| *fd >= 0).collect()
    })
}

/// The descriptors set in a `select` read set of `count` bits.
fn select_fds(pid: u32, count: u64, address: u64) -> Vec<i32> {
    let count = count.min(1024) as usize;
    if address == 0 {
        return vec![];
    }
    memory(pid, address, count.div_ceil(64) * 8).map_or_else(Vec::new, |bytes| {
        (0..count).filter(|bit| bytes[bit / 8] & (1 << (bit % 8)) != 0).map(|bit| bit as i32).collect()
    })
}

/// The descriptors an epoll instance watches (`tfd:` in its fdinfo).
fn epoll_fds(pid: u32, epfd: i32) -> Vec<i32> {
    fs::read_to_string(format!("/proc/{pid}/fdinfo/{epfd}")).map_or_else(|_| vec![], |info| {
        info.lines().filter_map(|line| line.strip_prefix("tfd:")?.split_whitespace().next()?.parse().ok()).collect()
    })
}

/// `rchar`.
pub(super) fn written_bytes(pid: u32) -> Option<u64> {
    fs::read_to_string(format!("/proc/{pid}/io"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("wchar: "))?
        .trim()
        .parse()
        .ok()
}

pub(super) fn become_subreaper() -> crate::Result<()> {
    set_child_subreaper(Some(getpid())).map_err(|error| crate::Error::os("become a subreaper")(error.into()))
}

/// Async-signal-safe: rustix's Linux backend makes these syscalls itself.
pub(super) fn die_with_parent(parent: u32) -> crate::Result<bool> {
    set_parent_process_death_signal(Some(Signal::KILL)).map_err(|error| crate::Error::os("die with the parent")(error.into()))?;
    Ok(Pid::as_raw(getppid()) as u32 == parent)
}

/// Exited, a zombie included, or gone altogether.
pub(super) fn exited(pid: u32) -> bool {
    match stat(pid) {
        Ok(stat) => stat.exited(),
        Err(error) => gone(&error),
    }
}

pub(super) fn in_kernel(pid: u32) -> Option<String> {
    let tid = fs::read_dir(format!("/proc/{pid}/task")).ok()?.flatten().find(|task| {
        let stat = fs::read_to_string(task.path().join("stat")).unwrap_or_default();
        stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('D'))
    })?;
    let wchan = fs::read_to_string(tid.path().join("wchan")).unwrap_or_default();
    Some(if wchan == "0" { String::new() } else { wchan })
}

pub(super) fn threads(pid: u32) -> Vec<super::Thread> {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return vec![] };
    let mut threads: Vec<super::Thread> = (tasks.flatten())
        .filter_map(|task| {
            let tid = task.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(task.path().join("stat")).ok()?;
            let (head, rest) = stat.rsplit_once(") ")?;
            let state = rest.chars().next()?;
            let name = head.split_once(" (")?.1.to_owned();
            let wchan = fs::read_to_string(task.path().join("wchan")).unwrap_or_default();
            let wchan = if wchan == "0" { String::new() } else { wchan.trim().to_owned() };
            (!matches!(state, 'Z' | 'X')).then_some(super::Thread { tid, name, state, wchan })
        })
        .collect();
    threads.sort_by_key(|thread| thread.tid);
    threads
}

pub(super) fn boot_id() -> io::Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .into())
}

/// `/proc/<pid>` vanished: ENOENT, or ESRCH when the process was reaped mid-read.
pub(super) fn gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

struct Stat {
    state: char,
    ppid: u32,
    pgrp: i32,
    /// The kernel's PF_* flags.
    flags: u64,
    threads: u64,
    start_ticks: u64,
}

impl Stat {
    /// Its last thread is gone, its memory and files with it: what a pidfd says. A leader that
    /// exits first shows `Z` while the others still hold them.
    fn exited(&self) -> bool {
        matches!(self.state, 'Z' | 'X') && self.threads <= 1
    }
}

fn stat(pid: u32) -> io::Result<Stat> {
    let record = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, suffix) = record
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    let fields: Vec<_> = suffix.split_whitespace().collect();
    let field = |index: usize| {
        fields
            .get(index)
            .copied()
            .ok_or_else(|| io::Error::other("process stat is truncated"))
    };
    Ok(Stat {
        state: field(0)?.chars().next().unwrap_or('?'),
        ppid: field(1)?.parse().map_err(io::Error::other)?,
        pgrp: field(2)?.parse().map_err(io::Error::other)?,
        flags: field(6)?.parse().map_err(io::Error::other)?,
        threads: field(17)?.parse().map_err(io::Error::other)?,
        start_ticks: field(19)?.parse().map_err(io::Error::other)?,
    })
}

fn sigkill_pending(pid: u32) -> io::Result<bool> {
    let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => status,
        Err(error) if gone(&error) => return Ok(true),
        Err(error) => return Err(error),
    };
    let kill = 1u64 << (libc::SIGKILL - 1);
    Ok(status
        .lines()
        .filter_map(|line| {
            line.strip_prefix("SigPnd:")
                .or_else(|| line.strip_prefix("ShdPnd:"))
        })
        .filter_map(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .any(|mask| mask & kill != 0))
}
