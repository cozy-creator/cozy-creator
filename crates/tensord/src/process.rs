//! Executor process lifetimes: groups, reaping and supervised helpers. The OS half (births,
//! exact handles, groups) is `platform::process`. Nothing here judges progress: a process ends
//! when it exits, or when its caller kills it (a cancel, an owner restart, a launch that never
//! reached authored code).
use platform::process::{Birth, Exact, State, gone, group_members, written_bytes};
use std::{
    fs::File,
    io,
    process::{Child, ExitStatus},
    time::Duration,
};

/// Whether a member of `leader`'s group born before `before` (start ticks) still lives: a
/// previous machine's follower, killed with its leader, until its exit is observed.
pub(crate) fn group_outlives(leader: &Birth, before: u64) -> bool {
    group_members(leader)
        .iter()
        .any(|member| member.start_ticks < before && member.state().is_ok_and(|state| state != State::Gone))
}

/// How often a wait on a killed process looks whether it is stuck in the kernel: never a deadline.
const SAMPLE: Duration = Duration::from_millis(100);

/// A killed process stuck in the kernel (D state): shown in the machine's status
/// (`platform::contain::stuck`), not waited on. Its own thread reaps it once it exits.
#[derive(Debug)]
pub(crate) struct Stuck(pub u32);
impl std::fmt::Display for Stuck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "pid {} was killed and is stuck in the kernel (D state): it holds what it held, \
            GPU memory included, until it exits", self.0)
    }
}
impl std::error::Error for Stuck {}

/// Wait for this exact process to exit (its caller closed its channel, or `killed` it), then reap
/// it. Then its scope: whatever outlived it (a `setsid` daemon) is killed and counted, and the
/// scope is removed. A killed one stuck in the kernel ends the wait with [`Stuck`].
pub(crate) fn reap(exact: &Exact, child: Option<Child>, killed: bool) -> io::Result<Reaped> {
    if !killed {
        exact.wait()?;
    }
    while killed && !exact.wait_for(SAMPLE)? {
        if platform::contain::all_stuck(std::slice::from_ref(exact.birth())) {
            let (exact, pid) = (exact.try_clone()?, exact.birth().pid);
            std::thread::Builder::new().name(format!("stuck-{pid}")).spawn(move || {
                if exact.wait().is_ok() {
                    let _ = child.map(|mut child| child.wait());
                    let _ = exact.scope().map(|scope| scope.end());
                }
            })?;
            return Err(io::Error::other(Stuck(pid)));
        }
    }
    let status = match child {
        Some(mut child) => child.wait()?,
        None => ExitStatus::default(),
    };
    // Read before its scope goes.
    let oom_killed = exact.scope().is_some_and(|scope| scope.oom_killed());
    let stragglers = match exact.scope() {
        Some(scope) => scope.end()?,
        None => 0,
    };
    Ok(Reaped { status, stragglers, oom_killed })
}

/// How a process ended: its status, and how many of its descendants were still alive in its
/// scope and were killed after it.
#[derive(Debug)]
pub(crate) struct Reaped {
    pub status: ExitStatus,
    pub stragglers: usize,
    /// Its scope saw the kernel's OOM killer end a member: the cause when it died by SIGKILL.
    pub oom_killed: bool,
}

/// `reap` the killed leader, then every member left in its group (followers die with their leader
/// by parent-death signal; one still alive is killed).
pub(crate) fn reap_group(exact: &Exact, child: Option<Child>) -> io::Result<Reaped> {
    // With a scope the leader's reap already ended its followers (they share it).
    let reaped = reap(exact, child, true)?;
    for member in group_members(exact.birth()) {
        if let Some(member) = Exact::open(&member)? {
            let _ = member.kill();
            reap(&member, None, true)?;
        }
    }
    Ok(reaped)
}

/// How a supervised process ended (`supervise`).
#[derive(Debug)]
pub(crate) enum Supervised {
    Exited(ExitStatus),
    /// Its run was canceled: its group was killed.
    Canceled,
}

/// Waits for `child`, the leader of its own process group, reading it once a second: `wrote`
/// sees the bytes its group has written (downloads landing, wheels unpacked; a network read or a
/// linked file counts no byte), and its group is killed when `stop` is canceled. The second is
/// resolution, never a verdict.
pub(crate) fn supervise(child: &mut Child, stop: Option<&crate::stop::Stop>, wrote: &dyn Fn(u64)) -> io::Result<Supervised> {
    let exact = match Birth::of(child.id()).map_err(io::Error::from) {
        Ok(birth) => Exact::open(&birth)?,
        Err(error) if gone(&error) => None,
        Err(error) => return Err(error),
    };
    let Some(exact) = exact else {
        return child.wait().map(Supervised::Exited);
    };
    let mut canceled = false;
    // What each process of the group wrote so far: a gone member keeps what it wrote.
    let mut writes = std::collections::HashMap::new();
    while !exact.wait_for(Duration::from_secs(1))? {
        if canceled {
            continue;
        }
        let group = std::iter::once(exact.birth().pid).chain(group_members(exact.birth()).into_iter().map(|m| m.pid));
        for (pid, bytes) in group.filter_map(|pid| Some((pid, written_bytes(pid)?))) {
            let held: &mut u64 = writes.entry(pid).or_default();
            *held = (*held).max(bytes);
        }
        wrote(writes.values().sum());
        if stop.is_some_and(|stop| stop.is_cancelled()) {
            exact.kill()?;
            canceled = true;
        }
    }
    let status = child.wait()?;
    Ok(if canceled { Supervised::Canceled } else { Supervised::Exited(status) })
}

/// The last bytes of a log, for a failure reason.
pub(crate) fn tail(path: &std::path::Path) -> String {
    tail_from(path, 0, MESSAGE_TAIL)
}

/// The stderr a failure message quotes; a triage bundle keeps [`crate::triage::MAX_STDERR`].
pub(crate) const MESSAGE_TAIL: u64 = 2 << 10;

/// The last `limit` bytes written at or after byte `from`.
pub(crate) fn tail_from(path: &std::path::Path, from: u64, limit: u64) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    use std::io::{Read, Seek};
    let length = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(io::SeekFrom::Start(length.saturating_sub(limit).max(from)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Where executor cores land (cr-275): `core_pattern`'s directory and the name each core starts with.
#[cfg(target_os = "linux")]
static CORES: std::sync::OnceLock<(std::path::PathBuf, String)> = std::sync::OnceLock::new();

/// Executor cores kept, once at start. A pod's `core_pattern` may name a directory its container lacks
/// (vast's /var/lib/vastai_kaalia/data/), which loses every core: it is made. Each core is capped at
/// half the disk free now, at most 32 GiB: the soft RLIMIT_CORE, which every executor inherits, is only
/// ever lowered, so a computer that keeps no cores keeps none.
pub(crate) fn keep_cores() {
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{Resource, getrlimit, setrlimit};
        let pattern = std::fs::read_to_string("/proc/sys/kernel/core_pattern").unwrap_or_default();
        let target = std::path::Path::new(pattern.trim());
        let place = target.parent().zip(target.file_name()).filter(|(dir, _)| {
            target.is_absolute() && !dir.to_string_lossy().contains('%')
        });
        let mut free = None;
        if let Some((dir, name)) = place {
            match std::fs::create_dir_all(dir) {
                Ok(()) => {
                    free = rustix::fs::statvfs(dir).ok().map(|disk| disk.f_bavail * disk.f_frsize);
                    let prefix = name.to_string_lossy().split('%').next().unwrap_or_default().to_string();
                    let _ = CORES.set((dir.to_path_buf(), prefix));
                }
                Err(error) => tracing::info!("executor cores: {} not made: {error}", dir.display()),
            }
        }
        let cap: u64 = free.map_or(32 << 30, |free| (free / 2).min(32 << 30));
        let mut limit = getrlimit(Resource::Core);
        if limit.current.is_none_or(|current| current > cap) {
            limit.current = Some(cap);
            let _ = setrlimit(Resource::Core, limit);
        }
        tracing::info!(
            "executor cores: core_pattern {}, at most {} MiB each",
            pattern.trim(),
            getrlimit(Resource::Core).current.map_or(u64::MAX, |bytes| bytes >> 20)
        );
    }
}

/// After an executor died by a fatal signal: when it left a core, the older ones go and the machine's
/// log names the one kept.
pub(crate) fn kept_core(status: &ExitStatus) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::ExitStatusExt;
        if !status.core_dumped() {
            return;
        }
        let Some((dir, prefix)) = CORES.get().filter(|(_, prefix)| !prefix.is_empty()) else {
            return tracing::warn!("an executor left a core where core_pattern sends it");
        };
        let mut cores: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix.as_str()))
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        cores.sort();
        let Some((_, newest)) = cores.pop() else { return };
        for (_, old) in cores {
            let _ = std::fs::remove_file(old);
        }
        tracing::warn!("an executor left its core at {} (the newest only is kept)", newest.display());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = status;
}
