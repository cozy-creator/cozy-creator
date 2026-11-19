//! A SQLite database's WAL emptied once rows go (cl-984, cl-1031): checkpointed into the database and truncated, so no page
//! image of a deleted row stays in it. One rule for every store that deletes what must leave no trace (TensorD's machine
//! journal, Calcifer's state.db).
//!
//! The store holds off its own readers while it empties (no read in flight), and the checkpoint never waits: a reader outside
//! the process that holds an older snapshot, or a write of its own in flight, leaves the emptying owed, said once. The store's
//! next write tries it again ([`Wal::retry`]), later ones at most once a second while it stays owed, and its next start does too.
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};

/// How often a write tries an owed emptying again after its first retry: a reader held open for hours never makes every write
/// wait on the store's reads.
const AGAIN: Duration = Duration::from_secs(1);

/// The WAL of one database, and whether an emptying is owed.
pub struct Wal {
    name: &'static str,
    owed: AtomicBool,
    /// When a write last retried the owed emptying, in ms of this process's clock (`now`); 0: not since it became owed.
    retried: AtomicU64,
}

impl Wal {
    /// The WAL of the database a person knows as `name`.
    pub const fn new(name: &'static str) -> Self {
        Self { name, owed: AtomicBool::new(false), retried: AtomicU64::new(0) }
    }

    /// Emptied now, on any connection to the database, while the caller holds off its own readers: false, and owed, while a
    /// reader outside them holds an older snapshot or a write is in flight.
    pub fn empty(&self, connection: &Connection) -> bool {
        // Owed before the checkpoint, not after: a busy TRUNCATE still copies and syncs what it can before it answers, and a write
        // that commits meanwhile must find it owed to retry it. A newly owed one is the next write's to retry at once.
        let was = self.owed.swap(true, Ordering::AcqRel);
        if !was {
            self.retried.store(0, Ordering::Release);
        }
        let emptied = truncate(connection);
        match (emptied, was) {
            (true, _) => {
                self.owed.store(false, Ordering::Release);
                if was {
                    tracing::info!("{}: its WAL, owed an emptying, is empty", self.name);
                }
            }
            (false, false) => tracing::warn!(
                "{}: its WAL still holds deleted rows' pages, as a reader outside this process holds an older snapshot; it's emptied after the next write",
                self.name
            ),
            (false, true) => {}
        }
        emptied
    }

    /// Whether this write should retry the owed emptying now, the retry taken by it: the first write after it became owed, then
    /// one a second at most.
    pub fn retry(&self) -> bool {
        if !self.owed.load(Ordering::Acquire) {
            return false;
        }
        let (last, now) = (self.retried.load(Ordering::Acquire), now());
        (last == 0 || now.saturating_sub(last) >= AGAIN.as_millis() as u64)
            && self.retried.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

/// Milliseconds of this process's own clock, from its first use; never 0.
fn now() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// The program outside this process that reads `database` from an older snapshot, as the OS says it without opening the
/// database's files (closing any descriptor of a file drops every POSIX lock this process holds on it, SQLite's own included):
/// its pid and name. SQLite's unix VFS holds a shared lock on one of the WAL index's read slots (`-shm` bytes 123..128) while a
/// reader reads. Linux lists every lock in `/proc/locks` by device and inode, which `stat` of the `-shm` names. None elsewhere,
/// or when only this process holds one (a write of its own was in flight).
pub fn holder(database: &Path) -> Option<(u32, String)> {
    holding(database)
}

#[cfg(target_os = "linux")]
fn holding(database: &Path) -> Option<(u32, String)> {
    use std::os::unix::fs::MetadataExt as _;
    let mut shm = database.as_os_str().to_owned();
    shm.push("-shm");
    let shm = std::fs::metadata(shm).ok()?;
    let dev = shm.dev();
    // As the kernel prints a device: its major and minor in hex, then the inode.
    let (major, minor) = (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff), (dev & 0xff) | ((dev >> 12) & !0xff));
    let file = format!("{major:02x}:{minor:02x}:{}", shm.ino());
    let own = std::process::id();
    let locks = std::fs::read_to_string("/proc/locks").ok()?;
    // `1: POSIX  ADVISORY  READ 1234 08:01:131074 123 123`; a waiter's line has `->` and holds nothing.
    let pid = locks.lines().filter(|line| !line.contains("->")).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [_, "POSIX", _, _, pid, at, start, end] = fields.as_slice() else { return None };
        let (pid, start) = (pid.parse::<u32>().ok()?, start.parse::<u64>().ok()?);
        let reaches = *end == "EOF" || end.parse::<u64>().is_ok_and(|end| end >= 123);
        (*at == file && pid != own && start <= 127 && reaches).then_some(pid)
    })?;
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|name| name.trim().to_owned()).unwrap_or_default();
    Some((pid, name))
}

#[cfg(not(target_os = "linux"))]
fn holding(_: &Path) -> Option<(u32, String)> {
    None
}

/// `wal_checkpoint(TRUNCATE)` once, never waiting on the connection's busy timeout: a reader that holds an older snapshot
/// answers it busy at once.
fn truncate(connection: &Connection) -> bool {
    let wait = connection.pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0)).map_or(0, |ms| ms.max(0).unsigned_abs());
    if connection.busy_timeout(Duration::ZERO).is_err() {
        return false;
    }
    let emptied = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0)).is_ok_and(|busy| busy == 0);
    let _ = connection.busy_timeout(Duration::from_millis(wait));
    emptied
}
