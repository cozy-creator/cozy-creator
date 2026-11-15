//! An executor's containment scope (Runtime `proctree.ExecutorScope`): its own cgroup-v2
//! below the machine's where the host delegates one, else a process-tree token. Either
//! reaches descendants that call `setsid` or double-fork. Lifecycle containment, not a
//! sandbox: same-UID code can move out of a delegated cgroup or scrub its token.
//!
//! A shared machine is also contained ([`contain`]): it and its executors in one cgroup with a
//! memory limit the kernel enforces, so a run that outgrows the computer is killed inside it. The
//! limit is varena's number ([`follow`]); this only applies it.
use super::{Stuck, TOKEN_ENV};
use super::host_memory::{available_outside, computer, events, number};
use crate::process::{Birth, Exact, State, gone, in_kernel};
use rustix::{
    event::{PollFd, PollFlags, Timespec},
    io::Errno,
};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

const ROOT: &str = "/sys/fs/cgroup";

/// The machine's own cgroup once [`contain`] made it.
static CONTAINED: OnceLock<Contained> = OnceLock::new();

struct Contained {
    /// `<own>/cozy-<namespace>`: `machine` (its processes) and every executor scope.
    root: PathBuf,
    written: Mutex<Option<Budget>>,
}

/// A budget below this much movement is not written again.
const MOVED: u64 = 64 << 20;

/// What a contained machine and its executors may use, as last written to their cgroup: varena's
/// number (`varena_ledger::host::machine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Budget {
    /// `memory.max`: past it, the kernel takes the machine's page cache back, then kills inside
    /// the machine. No `memory.high`: below a max it throttles a run that cannot shrink (pinned
    /// pages, no swap) for a minute before the kill (a 3 GiB hog under a 1.5 GiB max, 11-06).
    pub max: u64,
    /// What the computer keeps for its other programs.
    pub reserve: u64,
    /// What the machine and its executors use now (`memory.current`).
    pub used: u64,
}

/// How often an end looks at killed members still alive: never a deadline.
const SAMPLE: Duration = Duration::from_millis(100);

/// How long killed processes may show no sign of leaving before one in the kernel is called stuck
/// (cl-935). A CUDA process's exit frees its device memory and unpins its host memory in the driver,
/// its threads in D state while it does and while processes queue on the driver's lock: bercouli's
/// pid 1161 (24 GB of host memory, two GPUs) sat in `os_acquire_rwlock_write` at 100 ms and was
/// called stuck. The signs are any waited-on process's CPU time, threads and resident pages: they
/// leave through the same driver, so one moving is the driver moving.
pub const STALL: Duration = Duration::from_secs(60);

/// This process's live tree scopes: a token on no live scope is unclaimed.
static LIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Killed members an end found stuck in the kernel, until each exits.
static STUCK: Mutex<Vec<(Birth, Stuck)>> = Mutex::new(Vec::new());

/// Killed processes waited on while they leave, and when any of them last moved.
static LEAVING: Mutex<Leaving> = Mutex::new(Leaving { members: Vec::new(), moved: None });

/// Told of each process once it is found stuck (TensorD takes its GPUs out of service).
static ON_STUCK: Mutex<Option<std::sync::Arc<dyn Fn(&Stuck) + Send + Sync>>> = Mutex::new(None);

struct Leaving {
    members: Vec<Member>,
    moved: Option<Instant>,
}

struct Member {
    birth: Birth,
    killed_unix_ms: i64,
    /// Its resident bytes when first waited on.
    resident: u64,
    signs: Signs,
}

/// What moves while a process leaves.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Signs {
    cpu_ticks: u64,
    threads: u64,
    resident_pages: u64,
}

fn signs(pid: u32) -> Option<Signs> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    let field = |i: usize| fields.get(i).and_then(|f| f.parse::<u64>().ok());
    Some(Signs { cpu_ticks: field(11)? + field(12)?, threads: field(17)?, resident_pages: field(21)? })
}

/// Its resident bytes (`VmRSS`); 0 once its memory is gone.
fn resident(pid: u32) -> u64 {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    status.lines().find_map(|l| l.strip_prefix("VmRSS:")?.trim().strip_suffix(" kB")?.trim().parse::<u64>().ok()).unwrap_or(0) << 10
}

/// `hook` hears of each process found stuck, once, on the thread that found it; the latest replaces any earlier.
pub fn on_stuck(hook: impl Fn(&Stuck) + Send + Sync + 'static) {
    *ON_STUCK.lock().unwrap() = Some(std::sync::Arc::new(hook));
}

/// One executor's containment: a delegated cgroup-v2 of its own, else a process-tree token.
#[derive(Debug)]
pub struct Scope(Backend);

#[derive(Debug)]
enum Backend {
    /// One exact directory, protected against path reuse by its inode. `oom_before`: the host's
    /// OOM kills when it was made, read where its own `memory.events` is not.
    Cgroup { relative: String, inode: u64, oom_before: u64 },
    /// `<namespace>-<id>` in the leader's environment, inherited by every descendant that
    /// keeps it. An escaped one is adopted and reaped by the machine's supervisor (the
    /// subreaper); a `/proc` census finds it by its token wherever it is parented.
    Tree { token: String, oom_before: u64 },
}

/// The host's OOM kills so far (`/proc/vmstat` `oom_kill`, every kind).
fn host_oom_kills() -> u64 {
    fs::read_to_string("/proc/vmstat")
        .ok()
        .and_then(|text| text.lines().find_map(|l| l.strip_prefix("oom_kill ")?.trim().parse().ok()))
        .unwrap_or(0)
}

/// Where executor scopes go: the machine's own cgroup once contained, else this process's.
fn scopes_parent() -> io::Result<PathBuf> {
    match CONTAINED.get() {
        Some(contained) => Ok(contained.root.clone()),
        None => own_cgroup(),
    }
}

fn write(path: &Path, text: &str) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.write_all(text.as_bytes())
}

fn pids(cgroup: &Path) -> Vec<u32> {
    fs::read_to_string(cgroup.join("cgroup.procs"))
        .map(|text| text.lines().filter_map(|l| l.parse().ok()).collect())
        .unwrap_or_default()
}

/// This process, its descendants, and its supervisor (its parent, running the same program) with
/// theirs: the processes a machine's cgroup holds when nothing else shares it.
fn family() -> BTreeSet<u32> {
    let own = std::process::id();
    let mut family: BTreeSet<u32> = std::iter::once(own).chain(crate::process::descendants(own)).collect();
    let exe = |pid: u32| fs::read_link(format!("/proc/{pid}/exe")).ok();
    if let Ok(parent) = crate::process::parent_of(own)
        && parent > 1
        && exe(parent).is_some()
        && exe(parent) == exe(own)
    {
        family.insert(parent);
        family.extend(crate::process::descendants(parent));
    }
    family
}

/// Contains this machine: its processes move to `<own>/cozy-<namespace>/machine`, the memory
/// controller is enabled down to its executor scopes, and none of them swaps. Its budget is then
/// [`follow`]ed. The reason when the host gives no such hierarchy: read-only or v1 (a container),
/// the memory controller not delegated, or programs other than this machine's in its cgroup (it
/// moves no one else's).
pub fn contain(namespace: &str) -> std::result::Result<PathBuf, String> {
    if let Some(contained) = CONTAINED.get() {
        return Ok(contained.root.clone());
    }
    if !writable_unified() {
        return Err("no writable cgroup v2 hierarchy".into());
    }
    let own = own_cgroup().map_err(|error| format!("no cgroup of its own: {error}"))?;
    let name = format!("cozy-{namespace}");
    // A machine restarted in place is already inside.
    let inside = own.file_name().is_some_and(|n| n == "machine")
        && own.parent().and_then(Path::file_name).is_some_and(|n| n.to_string_lossy() == name);
    let root = if inside { own.parent().unwrap().to_path_buf() } else { own.join(&name) };
    let outer = root.parent().unwrap().to_path_buf();
    let failed = |what: &str, path: &Path, error: io::Error| format!("{what} {}: {error}", path.display());
    let delegated = fs::read_to_string(outer.join("cgroup.controllers")).map_err(|e| failed("read", &outer, e))?;
    if !delegated.split_whitespace().any(|c| c == "memory") {
        return Err(format!("the memory controller is not delegated to {}", outer.display()));
    }
    let machine = root.join("machine");
    if !inside {
        let ours = family();
        let others: Vec<String> = pids(&own)
            .into_iter()
            .filter(|pid| !ours.contains(pid))
            .map(|pid| format!("{pid} {}", fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim()))
            .collect();
        if !others.is_empty() {
            return Err(format!("{} also holds other programs ({})", own.display(), others.join(", ")));
        }
        for dir in [&root, &machine] {
            match fs::create_dir(dir) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(failed("create", dir, error)),
                _ => (),
            }
        }
        // A cgroup holding processes cannot enable a controller for its children: each moves to
        // the leaf, a fork made meanwhile too.
        let mut moved = 0;
        loop {
            for pid in pids(&own) {
                match write(&machine.join("cgroup.procs"), &pid.to_string()) {
                    Err(error) if error.raw_os_error() != Some(libc::ESRCH) => return Err(failed("move into", &machine, error)),
                    _ => (),
                }
            }
            match write(&outer.join("cgroup.subtree_control"), "+memory") {
                Err(error) if error.raw_os_error() == Some(libc::EBUSY) && moved < 8 => moved += 1,
                Err(error) => return Err(failed("enable memory under", &outer, error)),
                Ok(()) => break,
            }
        }
    }
    write(&root.join("cgroup.subtree_control"), "+memory").map_err(|e| failed("enable memory under", &root, e))?;
    // No swap accounting (a kernel built or booted without it): nothing of its swaps to bound.
    match write(&root.join("memory.swap.max"), "0") {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(failed("write memory.swap.max of", &root, error)),
        _ => (),
    }
    let _ = CONTAINED.set(Contained { root: root.clone(), written: Mutex::new(None) });
    Ok(root)
}

/// What the kernel accounts for a contained machine, for varena to size its limit from
/// (`varena_ledger::host::machine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineFacts {
    /// The computer: physical memory, or less where a limit above the machine's own is.
    pub computer: u64,
    /// What it has available outside the machine's cgroups.
    pub outside: u64,
    /// What the machine and its executors hold that the kernel cannot take back (page cache aside).
    pub held: u64,
    /// What the machine's own processes use (`machine/memory.current`).
    pub own: u64,
}

/// Applies varena's number to a contained machine's cgroup: `size` turns what the kernel accounts
/// into (`memory.max`, the reserve kept outside it). Written once it moved [`MOVED`]. None when not
/// contained. This sizes nothing.
pub fn follow(size: impl FnOnce(&MachineFacts) -> (u64, u64)) -> Option<io::Result<Budget>> {
    let contained = CONTAINED.get()?;
    let read = || -> io::Result<(MachineFacts, u64)> {
        let root = &contained.root;
        let (Ok(outside), Some(computer)) = (u64::try_from(available_outside()), computer()) else {
            return Err(io::Error::other("/proc/meminfo is unreadable"));
        };
        let used = number(&root.join("memory.current")).unwrap_or(0);
        let stat = fs::read_to_string(root.join("memory.stat"))?;
        let file: u64 = stat
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter(|(name, _)| matches!(*name, "active_file" | "inactive_file"))
            .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
            .sum();
        let own = number(&root.join("machine/memory.current")).unwrap_or(0);
        Ok((MachineFacts { computer, outside, held: used.saturating_sub(file), own }, used))
    };
    Some(read().and_then(|(facts, used)| {
        let (max, reserve) = size(&facts);
        let budget = Budget { max, reserve, used };
        let mut written = contained.written.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(was) = *written
            && was.max.abs_diff(budget.max) < MOVED
        {
            return Ok(Budget { used: budget.used, ..was });
        }
        write(&contained.root.join("memory.max"), &budget.max.to_string())?;
        *written = Some(budget);
        Ok(budget)
    }))
}

/// The budget last written, when the machine is contained.
pub fn budget() -> Option<Budget> {
    *CONTAINED.get()?.written.lock().unwrap_or_else(|e| e.into_inner())
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
    rustix::fs::statvfs(ROOT).is_ok_and(|stat| !stat.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY))
}

impl Scope {
    /// A fresh scope: a cgroup where the unified hierarchy is writable and delegated, else a
    /// token (read-only or v1 hierarchies: Docker, RunPod).
    pub fn create(namespace: &str) -> crate::Result<Self> {
        let id = &id()?;
        if let Some(scope) = Self::cgroup(namespace, id)? {
            return Ok(scope);
        }
        Self::inherited(namespace)
    }

    /// A process-tree token given to a command before its exec. Its descendants retain that
    /// ownership even if this machine dies; the next machine sweeps them before reusing files.
    pub fn inherited(namespace: &str) -> crate::Result<Self> {
        let id = id()?;
        let token = format!("{namespace}-{id}");
        LIVE.lock().unwrap().insert(token.clone());
        Ok(Self(Backend::Tree { token, oom_before: host_oom_kills() }))
    }

    fn cgroup(namespace: &str, id: &str) -> io::Result<Option<Self>> {
        if !writable_unified() {
            return Ok(None);
        }
        let path = scopes_parent()?.join(format!("cozy-executor-{namespace}-{id}"));
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
        // Contained: it never swaps, and the kernel's OOM killer ends all of it, not one member.
        if CONTAINED.get().is_some() {
            for (file, value) in [("memory.swap.max", "0"), ("memory.oom.group", "1")] {
                if let Err(error) = write(&path.join(file), value) {
                    tracing::warn!("executor scope {}: {file}: {error}", path.display());
                }
            }
        }
        Ok(Some(Self(Backend::Cgroup {
            relative: format!("/{}", path.strip_prefix(ROOT).unwrap().display()),
            inode: fs::metadata(&path)?.ino(),
            oom_before: host_oom_kills(),
        })))
    }

    /// Whether the kernel's OOM killer ended a member: its own `memory.events` counts it (the
    /// machine's limit or the host's); without one, only that the host killed something since it
    /// was made, which its caller pairs with a member's death by SIGKILL.
    pub fn oom_killed(&self) -> bool {
        let before = match &self.0 {
            Backend::Cgroup { relative, inode, oom_before } => {
                let own = Self::path(relative, *inode).and_then(|path| fs::read_to_string(path.join("memory.events")));
                if let Ok(text) = own {
                    return events(&text, &["oom_kill", "oom_group_kill"]) > 0;
                }
                *oom_before
            }
            Backend::Tree { oom_before, .. } => *oom_before,
        };
        host_oom_kills() > before
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
            Backend::Tree { token, .. } => Some((TOKEN_ENV, token)),
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
            Backend::Cgroup { relative, inode, .. } => vec![
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
    pub(crate) fn processes(&self) -> crate::Result<usize> {
        match &self.0 {
            Backend::Cgroup { relative, inode, .. } => Ok(procs(&Self::path(relative, *inode)?)?.len()),
            Backend::Tree { token, .. } => {
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
            Backend::Cgroup { relative, inode, .. } => Ok(OpenOptions::new()
                .write(true)
                .open(Self::path(relative, *inode)?.join("cgroup.procs"))?
                .write_all(pid.to_string().as_bytes())?),
            Backend::Tree { .. } => Ok(()),
        }
    }

    pub(crate) fn kill(&self) -> crate::Result<()> {
        match &self.0 {
            Backend::Cgroup { relative, inode, .. } => Ok(OpenOptions::new()
                .write(true)
                .open(Self::path(relative, *inode)?.join("cgroup.kill"))?
                .write_all(b"1")?),
            Backend::Tree { token, .. } => {
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
            Backend::Cgroup { relative, inode, .. } => {
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
            Backend::Tree { token, .. } => {
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

    /// Count actual surviving members, including live and previously stuck scopes. Unlike
    /// `sweep`, this is a post-retirement observation, not a count of signals sent.
    /// Best-effort Cozy lifecycle ownership, not an OS-wide process census: unreadable
    /// unrelated peers are omitted. Observed owned members and exact known stuck births
    /// still prevent retirement.
    pub fn namespace_processes(namespace: &str) -> crate::Result<usize> {
        if namespace.is_empty() || namespace.contains('/') {
            return Err(io::Error::other("invalid executor namespace").into());
        }
        let rows = census()?;
        let mut found: BTreeSet<u32> = members(&rows, |token| namespace_token(token, namespace))
            .filter(|row| row.alive)
            .map(|row| row.pid)
            .collect();
        let prefix = format!("cozy-executor-{namespace}-");
        for row in rows.iter().filter(|row| row.alive) {
            match fs::read_to_string(format!("/proc/{}/cgroup", row.pid)) {
                Ok(text) => {
                    if text.lines().any(|line| {
                        line.split_once("::").is_some_and(|(_, path)| {
                            path.split('/').any(|part| {
                                part.strip_prefix(&prefix)
                                    .is_some_and(|id| !id.is_empty() && !id.contains('-'))
                            })
                        })
                    }) {
                        found.insert(row.pid);
                    }
                }
                Err(error) if gone(&error) => (),
                Err(error) if found.contains(&row.pid) => return Err(error.into()),
                Err(_) => (), // No observed ownership: another same-UID program may be nondumpable.
            }
        }
        let stuck:Vec<Birth>=STUCK.lock().unwrap().iter().map(|(birth,_)|birth.clone()).collect();
        for birth in stuck {
            if !birth.ended()? { found.insert(birth.pid); }
        }
        Ok(found.len())
    }

    /// At machine start every scope of this namespace belongs to an earlier run: kill, wait
    /// for and remove each. Returns how many processes they still held.
    pub fn sweep(namespace: &str) -> crate::Result<usize> {
        let mut held = end_members(unclaimed(namespace))?;
        if !writable_unified() {
            return Ok(held);
        }
        let prefix = format!("cozy-executor-{namespace}-");
        for entry in fs::read_dir(scopes_parent()?)? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let scope = Self(Backend::Cgroup {
                relative: format!("/{}", entry.path().strip_prefix(ROOT).unwrap().display()),
                inode: entry.metadata()?.ino(),
                oom_before: 0,
            });
            held += scope.end()?;
        }
        Ok(held)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Backend::Tree { token, .. } = &self.0 {
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
        let mut poll = [PollFd::new(&events, PollFlags::PRI)];
        let timeout = sample.map(|sample| Timespec::try_from(sample).map_err(io::Error::other)).transpose()?;
        // Kernfs wakes it when the file changes.
        match rustix::event::poll(&mut poll, timeout.as_ref()) {
            Ok(0) => {
                let births: Vec<Birth> = procs(path)?.into_iter().filter_map(|pid| process_birth(pid).ok()).collect();
                if all_stuck(&births) {
                    return Ok(false);
                }
            }
            Err(Errno::INTR) | Ok(_) => (),
            Err(error) => return Err(error.into()),
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

/// Whether every one of these killed members is stuck in the kernel: each in D state, and no killed
/// process waited on has moved for [`STALL`]. A sample, called every [`SAMPLE`] or so by whoever
/// waits; until then each leaves and is waited on. One stuck is shown until it exits (`stuck`), and
/// nothing waits on it: it runs no code of its own again, and what it holds (memory, GPU memory as
/// NVML reads it, files, locks) stays held until it exits.
pub fn all_stuck(births: &[Birth]) -> bool {
    if births.is_empty() {
        return false;
    }
    let (now, wall) = (Instant::now(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64));
    let mut guard = LEAVING.lock().unwrap();
    let leaving = &mut *guard;
    leaving.members.retain(|m| process_birth(m.birth.pid).is_ok_and(|b| b.start_ticks == m.birth.start_ticks) && !threads(m.birth.pid).is_empty());
    for birth in births {
        if leaving.members.iter().any(|m| m.birth == *birth) {
            continue;
        }
        if let Some(signs) = signs(birth.pid) {
            leaving.members.push(Member { birth: birth.clone(), killed_unix_ms: wall, resident: resident(birth.pid), signs });
            leaving.moved = Some(now);
        }
    }
    for member in &mut leaving.members {
        if let Some(signs) = signs(member.birth.pid).filter(|signs| *signs != member.signs) {
            member.signs = signs;
            leaving.moved = Some(now);
        }
    }
    let quiet = now.duration_since(leaving.moved.unwrap_or(now));
    if quiet < STALL || !births.iter().all(|birth| in_kernel(birth.pid).is_some()) {
        return false;
    }
    let mut stuck = STUCK.lock().unwrap();
    let fresh: Vec<&Birth> = births.iter().filter(|birth| !stuck.iter().any(|(known, _)| known == *birth)).collect();
    let mut found = Vec::new();
    for birth in fresh {
        let wchan = in_kernel(birth.pid).unwrap_or_default();
        let command = fs::read_to_string(format!("/proc/{}/comm", birth.pid)).unwrap_or_default().trim().to_owned();
        let member = leaving.members.iter().find(|m| m.birth == *birth);
        let (since_unix_ms, resident_bytes) = member.map_or((wall, 0), |m| (m.killed_unix_ms, m.resident));
        tracing::warn!(
            "pid {} ({command}) is stuck in the kernel after its kill (D state, {wchan}): no killed process moved for {} s; \
             shown, not waited on", birth.pid, quiet.as_secs()
        );
        let shown = Stuck { pid: birth.pid, command, wchan, since_unix_ms, resident_bytes };
        stuck.push((birth.clone(), shown.clone()));
        found.push(shown);
    }
    drop((stuck, guard));
    let hook = ON_STUCK.lock().unwrap().clone();
    if let Some(hook) = hook {
        found.iter().for_each(|shown| hook(shown));
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

fn namespace_token(token: &str, namespace: &str) -> bool {
    token
        .strip_prefix(namespace)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|id| !id.is_empty() && !id.contains('-'))
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
