//! An executor's containment scope (Runtime `proctree.ExecutorScope`): its own cgroup-v2
//! below the machine's where the host delegates one, else a process-tree token. Either
//! reaches descendants that call `setsid` or double-fork. Lifecycle containment, not a
//! sandbox: same-UID code can move out of a delegated cgroup or scrub its token.
#![allow(unsafe_code)]
use super::{Stuck, TOKEN_ENV};
use crate::process::{Birth, Exact, State, gone, in_kernel};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

const ROOT: &str = "/sys/fs/cgroup";

/// How often an end looks at killed members still alive: never a deadline.
const SAMPLE: Duration = Duration::from_millis(100);

/// This process's live tree scopes: a token on no live scope is unclaimed.
static LIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Killed members an end found stuck in the kernel, until each exits.
static STUCK: Mutex<Vec<(Birth, Stuck)>> = Mutex::new(Vec::new());

#[derive(Debug)]
pub struct Scope(Backend);

#[derive(Debug)]
enum Backend {
    /// One exact directory, protected against path reuse by its inode.
    Cgroup { relative: String, inode: u64 },
    /// `<namespace>-<id>` in the leader's environment, inherited by every descendant that
    /// keeps it. An escaped one is adopted and reaped by the machine's supervisor (the
    /// subreaper); a `/proc` census finds it by its token wherever it is parented.
    Tree { token: String },
}

/// 16 random hex digits naming one scope.
fn id() -> io::Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex::encode(bytes))
}

fn process_birth(pid: u32) -> io::Result<Birth> {
    Birth::of(pid).map_err(io::Error::from)
}

fn process_ended(birth: &Birth) -> io::Result<bool> {
    Ok(birth.state()? == State::Gone)
}

fn own_cgroup() -> io::Result<PathBuf> {
    let entries = fs::read_to_string("/proc/self/cgroup")?;
    let relative = entries
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::other("no unified cgroup-v2 membership"))?;
    Ok(Path::new(ROOT).join(relative.trim().trim_start_matches('/')))
}

fn writable_unified() -> bool {
    if !Path::new(ROOT).join("cgroup.controllers").is_file() {
        return false;
    }
    let path = std::ffi::CString::new(ROOT).unwrap();
    // SAFETY: statvfs is plain old data; all-zero is a valid value.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid path and writable statvfs.
    unsafe { libc::statvfs(path.as_ptr(), &mut stat) == 0 && stat.f_flag & libc::ST_RDONLY == 0 }
}

impl Scope {
    /// A fresh scope: a cgroup where the unified hierarchy is writable and delegated, else a
    /// token (read-only or v1 hierarchies: Docker, RunPod).
    pub fn create(namespace: &str) -> crate::Result<Self> {
        let id = &id()?;
        if let Some(scope) = Self::cgroup(namespace, id)? {
            return Ok(scope);
        }
        let token = format!("{namespace}-{id}");
        LIVE.lock().unwrap().insert(token.clone());
        Ok(Self(Backend::Tree { token }))
    }

    fn cgroup(namespace: &str, id: &str) -> io::Result<Option<Self>> {
        if !writable_unified() {
            return Ok(None);
        }
        let path = own_cgroup()?.join(format!("cozy-executor-{namespace}-{id}"));
        if let Err(error) = fs::create_dir(&path) {
            return match error.kind() {
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Ok(None),
                _ => Err(error),
            };
        }
        // An undelegated hierarchy shows here, before any executor exists.
        let usable = ["cgroup.procs", "cgroup.kill"]
            .iter()
            .all(|name| OpenOptions::new().write(true).open(path.join(name)).is_ok())
            && path.join("cgroup.events").is_file();
        if !usable {
            let _ = fs::remove_dir(&path);
            return Ok(None);
        }
        Ok(Some(Self(Backend::Cgroup {
            relative: format!("/{}", path.strip_prefix(ROOT).unwrap().display()),
            inode: fs::metadata(&path)?.ino(),
        })))
    }

    /// Its cgroup (`/…`, as `0::` lines name it), when it is one.
    pub fn cgroup_relative(&self) -> Option<&str> {
        match &self.0 {
            Backend::Cgroup { relative, .. } => Some(relative),
            Backend::Tree { .. } => None,
        }
    }

    /// The entry its leader is launched with (a fork: in the environment it is forked with).
    pub fn environment(&self) -> Option<(&'static str, &str)> {
        match &self.0 {
            Backend::Cgroup { .. } => None,
            Backend::Tree { token } => Some((TOKEN_ENV, token)),
        }
    }

    fn path(relative: &str, inode: u64) -> io::Result<PathBuf> {
        let path = Path::new(ROOT).join(relative.trim_start_matches('/'));
        if fs::metadata(&path)?.ino() != inode {
            return Err(io::Error::other(format!(
                "executor cgroup {relative} was replaced"
            )));
        }
        Ok(path)
    }

    /// The trampoline joins a cgroup before it imports anything; a token needs nothing of it
    /// (the machine gives the process group).
    pub fn trampoline_args(&self) -> Vec<String> {
        match &self.0 {
            Backend::Cgroup { relative, inode } => vec![
                "--scope-backend".into(),
                "cgroup_v2".into(),
                "--cgroup".into(),
                relative.clone(),
                "--cgroup-inode".into(),
                inode.to_string(),
            ],
            Backend::Tree { .. } => vec!["--scope-backend".into(), "inherit".into()],
        }
    }

    /// Live processes in the scope.
    pub fn processes(&self) -> crate::Result<usize> {
        match &self.0 {
            Backend::Cgroup { relative, inode } => Ok(procs(&Self::path(relative, *inode)?)?.len()),
            Backend::Tree { token } => {
                let rows = census()?;
                Ok(members(&rows, |t| t == token)
                    .filter(|row| row.alive)
                    .count())
            }
        }
    }

    /// Move a running process into a cgroup scope (a fork starts in its parent's).
    pub fn adopt(&self, pid: u32) -> crate::Result<()> {
        match &self.0 {
            Backend::Cgroup { relative, inode } => Ok(OpenOptions::new()
                .write(true)
                .open(Self::path(relative, *inode)?.join("cgroup.procs"))?
                .write_all(pid.to_string().as_bytes())?),
            Backend::Tree { .. } => Ok(()),
        }
    }

    pub fn kill(&self) -> crate::Result<()> {
        match &self.0 {
            Backend::Cgroup { relative, inode } => Ok(OpenOptions::new()
                .write(true)
                .open(Self::path(relative, *inode)?.join("cgroup.kill"))?
                .write_all(b"1")?),
            Backend::Tree { token } => {
                let rows = census()?;
                for row in members(&rows, |t| t == token).filter(|row| row.alive) {
                    if let Some(exact) = row.exact()? {
                        exact.kill()?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Kill whatever outlived the executor (after its leader is reaped), wait for it, and
    /// remove the scope. Returns how many processes were still inside. Members stuck in the
    /// kernel are shown (`stuck`), not waited on: the scope is removed once they exit.
    pub fn end(&self) -> crate::Result<usize> {
        match &self.0 {
            Backend::Cgroup { relative, inode } => {
                let stragglers = self.processes()?;
                if stragglers > 0 {
                    self.kill()?;
                }
                let path = Self::path(relative, *inode)?;
                if wait_empty(&path, Some(SAMPLE))? {
                    remove_tree(&path)?;
                } else {
                    std::thread::Builder::new().name("scope-stuck".into()).spawn(move || {
                        if wait_empty(&path, None).is_ok() {
                            let _ = remove_tree(&path);
                        }
                    })?;
                }
                Ok(stragglers)
            }
            Backend::Tree { token } => {
                let stragglers = end_members(|t| t == token)?;
                LIVE.lock().unwrap().remove(token);
                // Anything of this machine's that no live scope claims goes now too.
                let namespace = token
                    .rsplit_once('-')
                    .map_or("", |(namespace, _)| namespace);
                Ok(stragglers + end_members(unclaimed(namespace))?)
            }
        }
    }

    /// At machine start every scope of this namespace belongs to an earlier run: kill, wait
    /// for and remove each. Returns how many processes they still held.
    pub fn sweep(namespace: &str) -> crate::Result<usize> {
        let mut held = end_members(unclaimed(namespace))?;
        if !writable_unified() {
            return Ok(held);
        }
        let prefix = format!("cozy-executor-{namespace}-");
        for entry in fs::read_dir(own_cgroup()?)? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let scope = Self(Backend::Cgroup {
                relative: format!("/{}", entry.path().strip_prefix(ROOT).unwrap().display()),
                inode: entry.metadata()?.ino(),
            });
            held += scope.end()?;
        }
        Ok(held)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Backend::Tree { token } = &self.0 {
            LIVE.lock().unwrap().remove(token);
        }
    }
}

/// Block until nothing in the cgroup is alive, woken by `cgroup.events`: true. With a `sample`,
/// false once a sample passes with every member left stuck in the kernel.
fn wait_empty(path: &Path, sample: Option<Duration>) -> io::Result<bool> {
    let mut events = File::open(path.join("cgroup.events"))?;
    loop {
        let mut text = String::new();
        events.rewind()?;
        events.read_to_string(&mut text)?;
        if text.lines().any(|line| line == "populated 0") {
            return Ok(true);
        }
        let mut poll = libc::pollfd {
            fd: events.as_raw_fd(),
            events: libc::POLLPRI,
            revents: 0,
        };
        let timeout = sample.map_or(-1, |sample| sample.as_millis() as i32);
        // SAFETY: one live descriptor; kernfs wakes it when the file changes.
        match unsafe { libc::poll(&mut poll, 1, timeout) } {
            0 => {
                let births: Vec<Birth> = procs(path)?.into_iter().filter_map(|pid| process_birth(pid).ok()).collect();
                if all_stuck(&births) {
                    return Ok(false);
                }
            }
            ready if ready < 0 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            _ => (),
        }
    }
}

/// Every process in the cgroup and its descendants.
fn procs(path: &Path) -> io::Result<Vec<u32>> {
    let mut pids: Vec<u32> = fs::read_to_string(path.join("cgroup.procs"))?
        .lines()
        .filter_map(|line| line.parse().ok())
        .collect();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            pids.extend(procs(&entry.path())?);
        }
    }
    Ok(pids)
}

/// Its threads still alive, each with its state (`R`, `S`, `D`, …): none once it exited.
fn threads(pid: u32) -> Vec<(u32, char)> {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| {
            let tid = task.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(task.path().join("stat")).ok()?;
            let state = stat.rsplit_once(") ")?.1.chars().next()?;
            (!matches!(state, 'Z' | 'X')).then_some((tid, state))
        })
        .collect()
}

/// Whether every one of these killed members is stuck in the kernel; each is then shown until it
/// exits (`stuck`), and nothing waits on it. It runs no code of its own again, and what it holds
/// (memory, GPU memory as NVML reads it, files, locks) stays held, and seen held, until it exits.
pub fn all_stuck(births: &[Birth]) -> bool {
    if births.is_empty() || !births.iter().all(|birth| in_kernel(birth.pid).is_some()) {
        return false;
    }
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut stuck = STUCK.lock().unwrap();
    let fresh: Vec<&Birth> = births.iter().filter(|birth| !stuck.iter().any(|(known, _)| known == *birth)).collect();
    for birth in fresh {
        let wchan = in_kernel(birth.pid).unwrap_or_default();
        let command = fs::read_to_string(format!("/proc/{}/comm", birth.pid)).unwrap_or_default().trim().to_owned();
        tracing::warn!("pid {} ({command}) is stuck in the kernel after its kill (D state, {wchan}): shown, not waited on", birth.pid);
        stuck.push((birth.clone(), Stuck { pid: birth.pid, command, wchan, since_unix_ms: now }));
    }
    true
}

/// Killed members still stuck in the kernel, each since an end found it.
pub fn stuck() -> Vec<Stuck> {
    let mut stuck = STUCK.lock().unwrap();
    stuck.retain(|(birth, _)| {
        process_birth(birth.pid).is_ok_and(|now| now.start_ticks == birth.start_ticks) && !threads(birth.pid).is_empty()
    });
    stuck.iter().map(|(_, shown)| shown.clone()).collect()
}

fn is_stuck(pid: u32, start_ticks: u64) -> bool {
    STUCK.lock().unwrap().iter().any(|(birth, _)| birth.pid == pid && birth.start_ticks == start_ticks)
}

fn remove_tree(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_tree(&entry.path())?;
        }
    }
    fs::remove_dir(path)
}

/// One process as a census reads it.
struct Row {
    pid: u32,
    ppid: u32,
    start_ticks: u64,
    alive: bool,
    token: Option<String>,
}
impl Row {
    fn exact(&self) -> io::Result<Option<Exact>> {
        match process_birth(self.pid) {
            Ok(birth) if birth.start_ticks == self.start_ticks => Ok(Exact::open(&birth)?),
            Ok(_) => Ok(None),
            Err(error) if gone(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// Every process but this one and init, with its scope token where its environment is
/// readable (a zombie has none).
fn census() -> io::Result<Vec<Row>> {
    let own = std::process::id();
    let mut rows = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Some(pid) = entry?
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own || pid == 1 {
            continue;
        }
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some((_, suffix)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<_> = suffix.split_whitespace().collect();
        let (Some(state), Some(Ok(ppid)), Some(Ok(start_ticks))) = (
            fields.first(),
            fields.get(1).map(|f| f.parse()),
            fields.get(19).map(|f| f.parse()),
        ) else {
            continue;
        };
        let alive = !matches!(*state, "Z" | "X");
        let token = alive
            .then(|| fs::read(format!("/proc/{pid}/environ")).ok())
            .flatten()
            .and_then(|environ| {
                environ.split(|b| *b == 0).find_map(|entry| {
                    let value = entry
                        .strip_prefix(TOKEN_ENV.as_bytes())?
                        .strip_prefix(b"=")?;
                    String::from_utf8(value.to_vec()).ok()
                })
            });
        rows.push(Row {
            pid,
            ppid,
            start_ticks,
            alive,
            token,
        });
    }
    Ok(rows)
}

/// Processes whose token `owned` accepts, and every descendant still parented below one.
fn members(rows: &[Row], owned: impl Fn(&str) -> bool) -> impl Iterator<Item = &Row> {
    let mut pids: BTreeSet<u32> = rows
        .iter()
        .filter(|row| row.token.as_deref().is_some_and(&owned))
        .map(|row| row.pid)
        .collect();
    loop {
        let before = pids.len();
        for row in rows {
            if pids.contains(&row.ppid) {
                pids.insert(row.pid);
            }
        }
        if pids.len() == before {
            break;
        }
    }
    rows.iter().filter(move |row| pids.contains(&row.pid))
}

/// A token of this namespace that no live scope of this process holds.
fn unclaimed(namespace: &str) -> impl Fn(&str) -> bool + '_ {
    move |token| {
        token
            .strip_prefix(namespace)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|id| !id.is_empty() && !id.contains('-'))
            && !LIVE.lock().unwrap().contains(token)
    }
}

/// Kill every member until a census finds none alive, then wait for each to exit, except one
/// stuck in the kernel (`all_stuck`). Its parent reaps it: an escaped one's is the supervisor.
fn end_members(owned: impl Fn(&str) -> bool) -> io::Result<usize> {
    let mut signaled: BTreeSet<(u32, u64)> = BTreeSet::new();
    let mut left: Vec<(Birth, Option<Exact>)> = Vec::new();
    loop {
        let rows = census()?;
        let fresh: Vec<&Row> = members(&rows, &owned)
            .filter(|row| row.alive && !signaled.contains(&(row.pid, row.start_ticks)))
            .filter(|row| !is_stuck(row.pid, row.start_ticks))
            .collect();
        if fresh.is_empty() {
            break;
        }
        for row in fresh {
            signaled.insert((row.pid, row.start_ticks));
            match row.exact()? {
                Some(exact) => {
                    exact.kill()?;
                    left.push((exact.birth().clone(), Some(exact)));
                }
                // A leader already exiting has no pidfd: its exit is sampled below.
                None => left.extend(process_birth(row.pid).ok().map(|birth| (birth, None))),
            }
        }
    }
    // The sample periods are how often an exit is looked for, never a deadline.
    let mut looked = Instant::now();
    loop {
        left.retain(|(birth, exact)| match exact {
            Some(exact) => !exact.ended(),
            None => !process_ended(birth).unwrap_or(true),
        });
        match left.first() {
            None => break,
            Some((_, Some(exact))) => {
                exact.wait_for(SAMPLE)?;
            }
            Some((_, None)) => std::thread::sleep(Duration::from_millis(10)),
        }
        if looked.elapsed() >= SAMPLE {
            looked = Instant::now();
            let births: Vec<Birth> = left.iter().map(|(birth, _)| birth.clone()).collect();
            if all_stuck(&births) {
                break;
            }
        }
    }
    Ok(signaled.len())
}
