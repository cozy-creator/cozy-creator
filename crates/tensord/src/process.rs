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
pub fn group_outlives(leader: &Birth, before: u64) -> bool {
    group_members(leader)
        .iter()
        .any(|member| member.start_ticks < before && member.state().is_ok_and(|state| state != State::Gone))
}

/// Wait for this exact process to exit (its caller killed it, or closed its channel), then reap
/// it. Then its scope: whatever outlived it (a `setsid` daemon) is killed and counted, and the
/// scope is removed.
pub fn reap(exact: &Exact, child: Option<&mut Child>) -> io::Result<Reaped> {
    exact.wait()?;
    let status = match child {
        Some(child) => child.wait()?,
        None => ExitStatus::default(),
    };
    let stragglers = match exact.scope() {
        Some(scope) => scope.end()?,
        None => 0,
    };
    Ok(Reaped { status, stragglers })
}

/// How a process ended: its status, and how many of its descendants were still alive in its
/// scope and were killed after it.
#[derive(Debug)]
pub struct Reaped {
    pub status: ExitStatus,
    pub stragglers: usize,
}

/// `reap` the leader, then every member left in its group (followers die with their leader by
/// parent-death signal; one still alive is killed).
pub fn reap_group(exact: &Exact, child: Option<&mut Child>) -> io::Result<Reaped> {
    // With a scope the leader's reap already ended its followers (they share it).
    let reaped = reap(exact, child)?;
    for member in group_members(exact.birth()) {
        if let Some(member) = Exact::open(&member)? {
            let _ = member.kill();
            reap(&member, None)?;
        }
    }
    Ok(reaped)
}

/// How a supervised process ended (`supervise`).
#[derive(Debug)]
pub enum Supervised {
    Exited(ExitStatus),
    /// Its run was canceled: its group was killed.
    Canceled,
}

/// Waits for `child`, the leader of its own process group, reading it once a second: `wrote`
/// sees the bytes its group has written (downloads landing, wheels unpacked; a network read or a
/// linked file counts no byte), and its group is killed when `stop` is canceled. The second is
/// resolution, never a verdict.
pub fn supervise(child: &mut Child, stop: Option<&crate::stop::Stop>, wrote: &dyn Fn(u64)) -> io::Result<Supervised> {
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
pub fn tail(path: &std::path::Path) -> String {
    tail_from(path, 0, MESSAGE_TAIL)
}

/// The stderr a failure message quotes; a triage bundle keeps [`crate::triage::MAX_STDERR`].
pub const MESSAGE_TAIL: u64 = 2 << 10;

/// The last `limit` bytes written at or after byte `from`.
pub fn tail_from(path: &std::path::Path, from: u64, limit: u64) -> String {
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
