//! One page of atomics per GPU, shared by every process on it: what each holds, who asked for
//! memory, who answered. Advisory: only the driver decides whether an allocation succeeds, and
//! nothing here promises bytes, so nothing can be counted twice. The worst an ask can do is
//! over-trim, which costs a refill.
//!
//! All zero is the valid start, and every word is written alone, so there is nothing to
//! initialize and nothing a dying writer can tear. A row is alive while its owner's OFD lock
//! is held; the kernel drops it when the process's last descriptor closes.
#![cfg(target_os = "linux")]
#![allow(unsafe_code)]

use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::{IntoRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};
use std::time::{Duration, Instant};

const ROWS: usize = 32;
const PAGE: usize = 4096;
/// Between asks: how often a donor republishes and samples.
const CADENCE: Duration = Duration::from_millis(500);
/// How often a waiter rechecks what no futex wake can report: a killed peer.
const TICK: Duration = Duration::from_millis(50);
/// TensorD always answers a short asker; this bounds a TensorD that cannot.
const SUPERVISOR_ANSWER: Duration = Duration::from_secs(30);
const REFUSED: u64 = u64::MAX;
/// Asking for less just means asking again: every small allocation of a full card would cost a
/// round trip through every peer.
const SMALLEST_ASK: u64 = 512 << 20;

#[derive(Default)]
#[repr(C)]
struct Words {
    /// Owner's pid; 0 is a free row. Nonzero with no lock held means its owner died.
    pid: AtomicU64,
    /// Physical bytes the owner has mapped, and the part not in use.
    held: AtomicU64,
    idle: AtomicU64,
    /// Bytes the owner is asking for now; 0 once it stops asking.
    want: AtomicU64,
    /// The last generation this row answered.
    answered: AtomicU64,
    /// CLOCK_MONOTONIC ns of its last request end, and the requests in flight.
    last_use: AtomicU64,
    running: AtomicU64,
    /// `released + 1` when peers could not help and it waits on the supervisor; `REFUSED` once
    /// the supervisor has nothing to end.
    short: AtomicU64,
    /// Bytes it has given back for asks, cumulative.
    donated: AtomicU64,
}

#[repr(C)]
struct Page {
    /// Bumped by every ask; donors wait on it.
    generation: AtomicU64,
    /// Bumped by every answer, release and death; askers and the supervisor wait on it.
    answers: AtomicU64,
    /// Bytes given back, plus one per death: any change is a reason to retry the driver.
    released: AtomicU64,
    rows: [Words; ROWS],
}
const _: () = assert!(size_of::<Page>() <= PAGE);

/// A row this process owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row(usize);

/// What an allocator does after an ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Memory came back, or may be coming: try the driver again and ask again if refused.
    Retry,
    /// Nothing more can come: try once more, then report out of memory.
    Final,
}

/// A process's side of asks. Called only on its one donor thread, with no allocator lock held.
pub trait Donor {
    /// Physical bytes held and the part not in use now. No driver call.
    fn holdings(&mut self) -> (u64, u64);
    /// Give back about `bytes` of idle memory, without waiting on memory in use.
    /// Returns the bytes actually released.
    fn trim(&mut self, bytes: u64) -> u64;
    /// Between asks, every `CADENCE`, or every `TICK` while it says the card is short: a program
    /// taking what was given is still short, and waits on the next give.
    fn tick(&mut self, table: &Table, row: Row, idle: u64) -> bool;
}

/// One process's view of one GPU's page. Lives until the process exits.
pub struct Table {
    page: &'static Page,
    fd: RawFd,
    own: AtomicIsize,
}

/// The canonical directory, private to this effective UID.
fn directory() -> io::Result<PathBuf> {
    // SAFETY: geteuid takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let path = PathBuf::from(format!("/dev/shm/cozy-varena-{uid}"));
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    let found = fs::symlink_metadata(&path)?;
    if !found.is_dir() || found.uid() != uid || found.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not this user's private directory", path.display())));
    }
    Ok(path)
}

/// A `GPU-…` or `MIG-…` name's 16 bytes, as CUDA reports them.
pub fn parse_uuid(name: &str) -> Option<[u8; 16]> {
    let (kind, rest) = name.split_once('-')?;
    if kind != "GPU" && kind != "MIG" { return None; }
    let hex: Vec<u8> = rest.bytes().filter(|byte| *byte != b'-').collect();
    if hex.len() != 32 { return None; }
    let mut out = [0; 16];
    for (byte, pair) in out.iter_mut().zip(hex.chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

impl Table {
    pub fn open(uuid: &[u8; 16]) -> io::Result<Self> {
        let name: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
        // The layout version is in the name: peers of another layout simply do not meet.
        let path = directory()?.join(format!("v6-{name}"));
        let open = || OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW).open(&path);
        // The mapping gets its own description: a mapping keeps its description alive, in a forked
        // child too, and a description that held the locks would keep a dead owner's row alive.
        let mapped = open()?;
        mapped.set_len(PAGE as u64)?;
        // SAFETY: a fresh shared mapping of an open file of this size, never unmapped.
        let mapping = unsafe { libc::mmap(std::ptr::null_mut(), PAGE, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, std::os::fd::AsRawFd::as_raw_fd(&mapped), 0) };
        if mapping == libc::MAP_FAILED { return Err(io::Error::last_os_error()); }
        // SAFETY: page-aligned, zero-initialized by the kernel or by peers' atomics, valid for the
        // process's life, and every field is an atomic.
        let page = unsafe { &*mapping.cast::<Page>() };
        arm_fork_hooks();
        hold_descriptors();
        let locks = open();
        let registered = locks.map(|file| {
            let fd = file.into_raw_fd();
            FDS.iter().find(|slot| slot.load(Relaxed) < 0).inspect(|slot| slot.store(fd, Relaxed));
            fd
        });
        release_descriptors();
        let fd = registered?;
        Ok(Self { page, fd, own: AtomicIsize::new(-1) })
    }

    fn lock_command(&self, command: i32, kind: i32, byte: usize) -> io::Result<i16> {
        // SAFETY: zero is valid for flock; OFD locks require l_pid 0.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = kind as i16;
        lock.l_whence = libc::SEEK_SET as i16;
        lock.l_start = byte as i64;
        lock.l_len = 1;
        // SAFETY: a live descriptor and a valid flock.
        if unsafe { libc::fcntl(self.fd, command, &mut lock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(lock.l_type)
    }
    fn lock(&self, byte: usize) -> bool {
        self.lock_command(libc::F_OFD_SETLK, libc::F_WRLCK, byte).is_ok()
    }
    fn unlock(&self, byte: usize) {
        let _ = self.lock_command(libc::F_OFD_SETLK, libc::F_UNLCK, byte);
    }
    /// Another description holds this byte. A failed check counts as held: never bury on doubt.
    fn held_elsewhere(&self, byte: usize) -> bool {
        self.lock_command(libc::F_OFD_GETLK, libc::F_WRLCK, byte).is_ok_and(|kind| kind != libc::F_UNLCK as i16)
    }

    /// Take a free or dead row. None when the table is full: this process then only asks nobody.
    pub fn claim(&self) -> Option<Row> {
        (0..ROWS).find(|&index| self.lock(index + 1)).map(|index| {
            self.clear(index);
            let row = &self.page.rows[index];
            row.answered.store(self.page.generation.load(Acquire), Release);
            row.last_use.store(now_ns(), Release);
            row.pid.store(u64::from(std::process::id()), Release);
            self.own.store(index as isize, Release);
            Row(index)
        })
    }

    /// Give the row up: this process keeps running with no cooperation.
    pub fn leave(&self, row: Row) {
        self.own.store(-1, Release);
        self.clear(row.0);
        self.unlock(row.0 + 1);
    }

    /// Reset a row whose lock we hold, telling everyone if a dead owner left it.
    fn clear(&self, index: usize) {
        let row = &self.page.rows[index];
        let held = row.held.swap(0, AcqRel);
        if row.pid.swap(0, AcqRel) != 0 {
            self.release(held.max(1));
        }
        for word in [&row.idle, &row.want, &row.running, &row.short, &row.donated] {
            word.store(0, Release);
        }
    }

    /// A process has fully exited, by its parent's word: what it held is the driver's again.
    pub fn exited(&self) {
        self.release(1);
    }

    fn release(&self, bytes: u64) {
        self.page.released.fetch_add(bytes, AcqRel);
        self.changed();
    }
    fn changed(&self) {
        self.page.answers.fetch_add(1, AcqRel);
        wake(&self.page.answers);
    }

    /// Free every row whose owner is gone. A row still locked, even by a process already dying,
    /// is simply not reapable yet: never an error for anyone else.
    pub fn reap(&self) {
        let own = self.own.load(Acquire);
        for (index, row) in self.page.rows.iter().enumerate() {
            if index as isize != own && row.pid.load(Acquire) != 0 && !self.held_elsewhere(index + 1) && self.lock(index + 1) {
                self.clear(index);
                self.unlock(index + 1);
            }
        }
    }

    pub fn publish(&self, row: Row, held: u64, idle: u64) {
        let row = &self.page.rows[row.0];
        row.held.store(held, Release);
        row.idle.store(idle, Release);
    }

    pub fn request_begin(&self, row: Row) {
        self.page.rows[row.0].running.fetch_add(1, AcqRel);
    }

    pub fn running(&self, row: Row) -> bool {
        self.page.rows[row.0].running.load(Acquire) != 0
    }

    pub fn request_end(&self, row: Row) {
        let row = &self.page.rows[row.0];
        row.last_use.store(now_ns(), Release);
        let _ = row.running.try_update(AcqRel, Acquire, |running| running.checked_sub(1));
    }

    /// Bytes idle on this GPU, across every row.
    pub fn idle(&self) -> u64 {
        self.page.rows.iter().filter(|row| row.pid.load(Acquire) != 0).map(|row| row.idle.load(Acquire)).sum()
    }

    /// What every live row holds: the card's use less this is not ours.
    pub fn held(&self) -> u64 {
        self.page.rows.iter().filter(|row| row.pid.load(Acquire) != 0).map(|row| row.held.load(Acquire)).sum()
    }

    /// The pids in live rows: on this GPU, but not foreign.
    pub fn pids(&self) -> Vec<u32> {
        self.page.rows.iter().map(|row| row.pid.load(Acquire) as u32).filter(|pid| *pid != 0).collect()
    }

    /// Ask every donor to work out its share of `bytes`. The want stays up until cleared.
    pub fn ask(&self, row: Row, bytes: u64) -> u64 {
        self.page.rows[row.0].want.store(bytes, Release);
        let generation = self.page.generation.fetch_add(1, AcqRel) + 1;
        wake(&self.page.generation);
        generation
    }

    pub fn stop_asking(&self, row: Row) {
        let row = &self.page.rows[row.0];
        row.want.store(0, Release);
        row.short.store(0, Release);
    }

    /// This row's part of the largest want: idle processes first, oldest use first, then
    /// processes in a request. Every donor sorts the same way, so shares rarely overlap, and
    /// an overlap only over-trims.
    fn share(&self, me: Row) -> u64 {
        let live = || self.page.rows.iter().enumerate().filter(|(_, row)| row.pid.load(Acquire) != 0);
        let mut left = live().map(|(_, row)| row.want.load(Acquire)).max().unwrap_or(0);
        let mut order: Vec<_> = live().map(|(index, row)| (row.running.load(Acquire) != 0, row.last_use.load(Acquire), index)).collect();
        order.sort_unstable();
        for (_, _, index) in order {
            let take = left.min(self.page.rows[index].idle.load(Acquire));
            if index == me.0 { return take; }
            left -= take;
        }
        0
    }

    /// A process's one donor thread. Trims when asked and answers every generation. Never returns.
    pub fn serve(&self, row: Row, donor: &mut impl Donor) {
        let mut answered = self.page.rows[row.0].answered.load(Acquire);
        let mut sample = Instant::now();
        loop {
            let generation = self.page.generation.load(Acquire);
            if generation != answered {
                self.reap();
                let (held, idle) = donor.holdings();
                self.publish(row, held, idle);
                let share = self.share(row);
                let released = if share == 0 { 0 } else { donor.trim(share) };
                let (held, idle) = donor.holdings();
                self.publish(row, held, idle);
                let words = &self.page.rows[row.0];
                words.donated.fetch_add(released, AcqRel);
                words.answered.store(generation, Release);
                if released != 0 { self.page.released.fetch_add(released, AcqRel); }
                self.changed();
                answered = generation;
                continue;
            }
            let now = Instant::now();
            if now >= sample {
                let (held, idle) = donor.holdings();
                self.publish(row, held, idle);
                sample = now + if donor.tick(self, row, idle) { TICK } else { CADENCE };
                continue;
            }
            wait(&self.page.generation, generation, sample - now);
        }
    }

    /// The allocating thread, holding no lock: ask and wait until memory comes back or nothing
    /// more can. `deadline` bounds waiting on peers that stopped answering. Each `Retry` is real
    /// progress, so a caller may give the next call a fresh deadline.
    pub fn recover(&self, row: Row, bytes: u64, deadline: Instant) -> Recovery {
        let result = self.recover_inner(row, bytes, deadline);
        self.stop_asking(row);
        result
    }

    fn recover_inner(&self, row: Row, bytes: u64, deadline: Instant) -> Recovery {
        let page = self.page;
        let before = page.released.load(Acquire);
        let bytes = bytes.max(SMALLEST_ASK);
        let mut generation = self.ask(row, bytes);
        let mut supervisor: Option<Instant> = None;
        let mut idle = u64::MAX;
        loop {
            let seen = page.answers.load(Acquire);
            self.reap();
            if page.released.load(Acquire) != before { return Recovery::Retry; }
            let now = Instant::now();
            if let Some(limit) = supervisor {
                // Another thread of this process finishing its own recovery clears the row's word.
                let short = &page.rows[row.0].short;
                let _ = short.compare_exchange(0, before + 1, AcqRel, Acquire);
                if short.load(Acquire) == REFUSED || now >= limit || !self.held_elsewhere(0) {
                    return Recovery::Final;
                }
            } else {
                let live = || page.rows.iter().enumerate().filter(|(_, words)| words.pid.load(Acquire) != 0);
                let answered = live().all(|(_, words)| words.answered.load(Acquire) >= generation);
                let peers = || live().filter(|(index, _)| *index != row.0);
                let left = peers().map(|(_, words)| words.idle.load(Acquire)).sum::<u64>();
                let finishing = peers().any(|(_, words)| words.running.load(Acquire) != 0 && words.idle.load(Acquire) != 0);
                if answered && now < deadline && left != 0 && (finishing || left != idle) {
                    // A running peer may be finishing a stream. An idle one that answered with the same
                    // figure twice has nothing more to give, however long this asks.
                    idle = left;
                    wait(&page.answers, seen, TICK);
                    generation = self.ask(row, bytes);
                    continue;
                }
                if answered || now >= deadline {
                    if !self.held_elsewhere(0) { return Recovery::Final; }
                    page.rows[row.0].short.store(before + 1, Release);
                    self.changed();
                    supervisor = Some(now + SUPERVISOR_ANSWER);
                    continue;
                }
            }
            let limit = if supervisor.is_some() { TICK } else { TICK.min(deadline.saturating_duration_since(now)) };
            wait(&page.answers, seen, limit);
        }
    }

    /// Another process holds idle memory and is in no request: nothing is about to use it.
    pub fn peers_idle(&self, row: Row) -> bool {
        self.page.rows.iter().enumerate().any(|(index, words)| index != row.0 && words.pid.load(Acquire) != 0
            && words.running.load(Acquire) == 0 && words.idle.load(Acquire) != 0)
    }

    /// Before this process evicts its own weights: one ask, and whether memory came back. Nothing
    /// final hangs on it, so no re-asking and no supervisor.
    pub fn ask_idle_peers(&self, row: Row, bytes: u64, deadline: Instant) -> bool {
        let before = self.page.released.load(Acquire);
        let generation = self.ask(row, bytes.max(SMALLEST_ASK));
        let gave = loop {
            let seen = self.page.answers.load(Acquire);
            self.reap();
            if self.page.released.load(Acquire) != before { break true; }
            let live = |words: &Words| words.pid.load(Acquire) != 0;
            let now = Instant::now();
            if now >= deadline || self.page.rows.iter().all(|words| !live(words) || words.answered.load(Acquire) >= generation) {
                break false;
            }
            wait(&self.page.answers, seen, TICK.min(deadline - now));
        };
        self.stop_asking(row);
        gave
    }

    /// An ask no allocator of this process waits on, as TensorD's for a process in no row (an
    /// executor older than the ledger) about to use this GPU: every donor gives back its share of
    /// `bytes` (`u64::MAX`: all it holds idle). A borrowed row, which answers other asks with nothing
    /// meanwhile, waits until each donor has answered or `deadline`. Needs its own `Table`: a claim
    /// takes the one row this table's `reap` spares. The bytes given back; None with no free row.
    pub fn ask_idle(&self, bytes: u64, deadline: Instant) -> Option<u64> {
        let row = self.claim()?;
        let before = self.page.released.load(Acquire);
        let generation = self.ask(row, bytes);
        loop {
            let seen = self.page.answers.load(Acquire);
            self.reap();
            self.page.rows[row.0].answered.store(self.page.generation.load(Acquire), Release);
            let now = Instant::now();
            let waiting = self.page.rows.iter().any(|words| words.pid.load(Acquire) != 0 && words.answered.load(Acquire) < generation);
            if !waiting || now >= deadline { break; }
            wait(&self.page.answers, seen, TICK.min(deadline - now));
        }
        let given = self.page.released.load(Acquire) - before;
        self.leave(row);
        Some(given)
    }

    /// TensorD's side, for this GPU. While the table's supervisor byte is held, an asker that
    /// peers could not help waits for `end_idle`: it ends an idle process, or says there is none.
    /// Only an exit returns a CUDA context. `end_idle` returns None when the service is gone.
    /// An asker's own process is never the one ended: its exit can't give it room.
    pub fn supervise(&self, mut end_idle: impl FnMut(&[u32]) -> Option<bool>) {
        if !self.lock(0) { return; } // Another TensorD on this GPU has it.
        loop {
            let seen = self.page.answers.load(Acquire);
            self.reap();
            let released = self.page.released.load(Acquire);
            let is_short = |row: &&Words| row.pid.load(Acquire) != 0 && row.short.load(Acquire) == released + 1;
            let askers: Vec<u32> = self.page.rows.iter().filter(is_short).map(|row| row.pid.load(Acquire) as u32).collect();
            if !askers.is_empty() {
                match end_idle(&askers) {
                    Some(true) => {
                        self.reap();
                        self.release(1);
                    }
                    Some(false) => {
                        for row in self.page.rows.iter().filter(is_short) {
                            let _ = row.short.compare_exchange(released + 1, REFUSED, AcqRel, Acquire);
                        }
                        self.changed();
                    }
                    None => return,
                }
                continue;
            }
            wait(&self.page.answers, seen, Duration::from_secs(1));
        }
    }
}

fn now_ns() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid output; CLOCK_MONOTONIC is shared by every process of one boot.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

// Futex words are the low half of a little-endian u64.
fn wait(word: &AtomicU64, seen: u64, timeout: Duration) {
    let time = libc::timespec { tv_sec: timeout.as_secs() as i64, tv_nsec: i64::from(timeout.subsec_nanos()) };
    // SAFETY: a live, aligned shared word. The kernel compares and sleeps atomically; any return is fine.
    unsafe { libc::syscall(libc::SYS_futex, word.as_ptr().cast::<u32>(), libc::FUTEX_WAIT, seen as u32, &time, 0usize, 0u32) };
}

fn wake(word: &AtomicU64) {
    // SAFETY: a live, aligned shared word.
    unsafe { libc::syscall(libc::SYS_futex, word.as_ptr().cast::<u32>(), libc::FUTEX_WAKE, i32::MAX, 0usize, 0usize, 0u32) };
}

// A forked child must not keep a parent's locks alive past its death: it closes the copies,
// which never unlocks the parent's. `exec` is covered by O_CLOEXEC.
static FDS: [AtomicI32; 64] = [const { AtomicI32::new(-1) }; 64];
static OPENING: AtomicBool = AtomicBool::new(false);

fn hold_descriptors() {
    while OPENING.swap(true, Acquire) { std::hint::spin_loop(); }
}
fn release_descriptors() {
    OPENING.store(false, Release);
}
extern "C" fn fork_prepare() { hold_descriptors(); }
extern "C" fn fork_parent() { release_descriptors(); }
extern "C" fn fork_child() {
    for slot in &FDS {
        let fd = slot.swap(-1, Relaxed);
        // SAFETY: an inherited table descriptor, closed once.
        if fd >= 0 { unsafe { libc::close(fd) }; }
    }
    release_descriptors();
}

fn arm_fork_hooks() {
    static ARMED: Once = Once::new();
    // SAFETY: the handlers only touch atomics and close descriptors. A registration failure
    // (ENOMEM) costs only that a forked child can delay a dead parent's row being freed.
    ARMED.call_once(|| { unsafe { libc::pthread_atfork(Some(fork_prepare), Some(fork_parent), Some(fork_child)) }; });
}
