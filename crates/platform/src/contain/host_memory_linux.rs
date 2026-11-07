//! Linux: `/proc/meminfo`, every cgroup on this process's path, and `smaps_rollup`.
use super::{HostMemory, ProcessMemory};
use std::{
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
};

/// The host's `MemTotal`; -1 when unreadable.
pub(super) fn total() -> i64 {
    meminfo("MemTotal")
}

pub(crate) fn meminfo(field: &str) -> i64 {
    let Ok(file) = fs::File::open("/proc/meminfo") else {
        return -1;
    };
    for line in io::BufReader::new(file).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix(field).and_then(|r| r.strip_prefix(':')) {
            return rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<i64>().ok())
                .map_or(-1, |kib| kib * 1024);
        }
    }
    -1
}

/// (`SwapTotal`, `SwapFree`); 0 where unreadable.
pub(super) fn swap() -> (u64, u64) {
    let read = |field| u64::try_from(meminfo(field)).unwrap_or(0);
    (read("SwapTotal"), read("SwapFree"))
}

pub(super) fn stall() -> super::Stall {
    let avg10 = |path: &Path| -> Option<f32> {
        let text = fs::read_to_string(path).ok()?;
        let some = text.lines().find(|line| line.starts_with("some "))?;
        some.split_whitespace().find_map(|field| field.strip_prefix("avg10="))?.parse().ok()
    };
    let limited = match cgroups() {
        Some((path, limits, "memory.current")) => path
            .into_iter()
            .filter(|cgroup| tightest(std::slice::from_ref(cgroup), limits).is_some())
            .map(|cgroup| cgroup.join("memory.pressure"))
            .collect(),
        _ => vec![],
    };
    let some_avg10 = std::iter::once(PathBuf::from("/proc/pressure/memory"))
        .chain(limited)
        .filter_map(|path| avg10(&path))
        .fold(0.0, f32::max);
    super::Stall { some_avg10 }
}

/// The sum of `names`' counts in a `memory.events`.
pub(crate) fn events(text: &str, names: &[&str]) -> u64 {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(name, _)| names.contains(name))
        .filter_map(|(_, count)| count.trim().parse::<u64>().ok())
        .sum()
}

pub(crate) fn number(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    // cgroup v2 spells "unlimited" `max`; v1 a page-rounded value near LONG_MAX
    text.parse::<u64>().ok().filter(|&n| n > 0 && n < (1 << 60))
}

/// (reclaimable, active, `shmem`) from a cgroup's `memory.stat`: the page cache the kernel gives
/// back before it OOM-kills is its inactive file pages less those still dirty or under writeback
/// (active ones may not go in time, dirty ones wait on the disk); active is the clean active file
/// pages beside it; v1 states its subtree's under `total_`.
fn stat(cgroup: &Path) -> Option<(u64, u64, u64)> {
    let text = fs::read_to_string(cgroup.join("memory.stat")).ok()?;
    let row = |names: &[&str]| -> u64 {
        let find = |key: &str| {
            text.lines().find_map(|l| {
                l.strip_prefix(key)
                    .and_then(|r| r.strip_prefix(' '))
                    .and_then(|v| v.trim().parse().ok())
            })
        };
        names.iter().find_map(|name| find(&format!("total_{name}")).or_else(|| find(name))).unwrap_or(0)
    };
    let unwritten = row(&["file_dirty", "dirty"]) + row(&["file_writeback", "writeback"]);
    let inactive = row(&["inactive_file"]);
    let active = row(&["active_file"]).saturating_sub(unwritten.saturating_sub(inactive));
    Some((inactive.saturating_sub(unwritten), active, row(&["shmem"])))
}

/// A process's memory cgroup and every parent up to the controller root, innermost first, with
/// the limit files and the usage file of its version.
pub(crate) type Cgroups = (Vec<PathBuf>, &'static [&'static str], &'static str);

/// This process's [`Cgroups`].
pub(crate) fn cgroups() -> Option<Cgroups> {
    let memberships = fs::read_to_string("/proc/self/cgroup").ok()?;
    cgroups_in(Path::new("/sys/fs/cgroup"), &memberships, || fs::read_to_string("/proc/self/mountinfo").ok())
}

/// The [`Cgroups`] of `memberships` (a `/proc/<pid>/cgroup`) under the cgroup mount `root`;
/// v1's memory controller is found in `mounts` (a `/proc/<pid>/mountinfo`).
fn cgroups_in(root: &Path, memberships: &str, mounts: impl FnOnce() -> Option<String>) -> Option<Cgroups> {
    let (leaf, top, limits, usage): (PathBuf, PathBuf, &[&str], &str) =
        if root.join("cgroup.controllers").is_file() {
            let relative = memberships.lines().find_map(|l| l.strip_prefix("0::"))?;
            (
                root.join(relative.trim_start_matches('/')),
                root.to_path_buf(),
                &["memory.high", "memory.max"],
                "memory.current",
            )
        } else {
            let relative = memberships.lines().find_map(|l| {
                let mut fields = l.splitn(3, ':');
                let (_, names, path) = (fields.next()?, fields.next()?, fields.next()?);
                names.split(',').any(|n| n == "memory").then_some(path)
            })?;
            // The memory controller's mount: its root (mountinfo field 4) is the part of the
            // membership path the mount hides (a container sees its own cgroup at the mount).
            let mounts = mounts()?;
            let (mount_root, point) = mounts.lines().find_map(|l| {
                let fields: Vec<&str> = l.split(' ').collect();
                let dash = fields.iter().position(|f| *f == "-")?;
                (fields.get(dash + 1) == Some(&"cgroup")
                    && fields.get(dash + 3)?.split(',').any(|o| o == "memory"))
                .then(|| (fields[3].to_string(), PathBuf::from(fields[4])))
            })?;
            let inside = relative
                .strip_prefix(mount_root.trim_end_matches('/'))
                .unwrap_or(relative);
            (
                point.join(inside.trim_start_matches('/')),
                point,
                &["memory.limit_in_bytes"],
                "memory.usage_in_bytes",
            )
        };
    let mut path = Vec::new();
    let mut at = Some(leaf.as_path());
    while let Some(cgroup) = at {
        path.push(cgroup.to_path_buf());
        if cgroup == top {
            break;
        }
        at = cgroup.parent().filter(|p| p.starts_with(&top));
    }
    Some((path, limits, usage))
}

/// The tightest limit set on any cgroup of `path`: None where none is.
fn tightest(path: &[PathBuf], limits: &[&str]) -> Option<u64> {
    path.iter().flat_map(|cgroup| limits.iter().filter_map(|name| number(&cgroup.join(name)))).min()
}

/// The tightest cgroup limit on this process's path.
pub(super) fn limit() -> Option<u64> {
    cgroups().and_then(|(path, limits, _)| tightest(&path, limits))
}

/// The live budget: the tightest of every cgroup on this process's path (each `memory.high`
/// and `memory.max`, less what that cgroup uses and cannot give back; inactive page cache is
/// reclaimable; a missing limit file is no limit, and a limit whose use is unreadable is all
/// room) and the host's `MemAvailable`.
pub(super) fn read() -> HostMemory {
    let mem_available = meminfo("MemAvailable");
    let mut best = HostMemory {
        available: mem_available,
        shmem: meminfo("Shmem").max(0),
        mem_available,
        bound: total(),
        active: 0,
    };
    let Some((path, limits, usage)) = cgroups() else {
        return best;
    };
    for cgroup in path {
        let Some(bound) = tightest(std::slice::from_ref(&cgroup), limits) else {
            continue;
        };
        let used = number(&cgroup.join(usage)).unwrap_or(0);
        let (cache, active, shmem) = stat(&cgroup).unwrap_or((0, 0, 0));
        let room = bound.saturating_sub(used.saturating_sub(cache)) as i64;
        if best.available < 0 || room < best.available {
            best = HostMemory {
                available: room,
                shmem: shmem as i64,
                mem_available,
                bound: bound as i64,
                active: active as i64,
            };
        }
    }
    best
}

/// A cgroup of a machine's own containment (`cozy-<namespace>`, `cozy-executor-…`) or below one:
/// its limits are the machine's, never the computer's.
fn ours(cgroup: &Path) -> bool {
    cgroup.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        name.starts_with("cozy-executor-")
            || name.strip_prefix("cozy-").is_some_and(|ns| ns.len() == 16 && ns.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

/// The computer this machine runs on: physical memory, or less where a limit above the machine's
/// own cgroups is (a container, a unit's `MemoryMax`). None when unreadable.
pub(crate) fn computer() -> Option<u64> {
    let physical = u64::try_from(total()).ok().filter(|&n| n > 0);
    let limits = cgroups().and_then(|(path, limits, _)| {
        let theirs: Vec<PathBuf> = path.into_iter().filter(|cgroup| !ours(cgroup)).collect();
        tightest(&theirs, limits)
    });
    physical.into_iter().chain(limits).min()
}

/// What the computer has available outside this machine's own cgroups: `MemAvailable`, or less
/// where a limit above them has less room. -1 when unreadable.
pub(crate) fn available_outside() -> i64 {
    let mut available = meminfo("MemAvailable");
    let Some((path, limits, usage)) = cgroups() else {
        return available;
    };
    for cgroup in path.iter().filter(|cgroup| !ours(cgroup)) {
        let Some(bound) = tightest(std::slice::from_ref(cgroup), limits) else {
            continue;
        };
        let used = number(&cgroup.join(usage)).unwrap_or(0);
        let (cache, _, _) = stat(cgroup).unwrap_or((0, 0, 0));
        let room = bound.saturating_sub(used.saturating_sub(cache)) as i64;
        if available < 0 || room < available {
            available = room;
        }
    }
    available
}

/// `/proc/<pid>/status`'s `RssAnon`.
pub(super) fn resident(pid: u32) -> io::Result<super::Resident> {
    let text = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':')?.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kib| kib * 1024)
            .ok_or_else(|| io::Error::other(format!("no {name} in /proc/{pid}/status")))
    };
    Ok(super::Resident { anon: field("RssAnon")? })
}

pub(super) fn process(pid: u32) -> io::Result<ProcessMemory> {
    let text = fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))?;
    let field = |name: &str| {
        text.lines()
            .find_map(|l| {
                l.strip_prefix(name)?
                    .strip_prefix(':')?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .map_or(0, |kib| kib * 1024)
    };
    Ok(ProcessMemory {
        rss: field("Rss"),
        pss: field("Pss"),
        pss_shmem: field("Pss_Shmem"),
    })
}
