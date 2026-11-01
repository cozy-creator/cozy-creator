//! Scoped Linux cooperative metadata. This module performs no CUDA operation and grants no memory.
//! See `ledger-foundation.md` for the supported namespace and fork contract.

mod os;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub const ROWS: usize = 64;
const MAGIC: u64 = u64::from_le_bytes(*b"VARENA3\0");
const VERSION: u64 = 1;
const HEADER: usize = 16;
const SELECTOR: usize = 15;
const ROW_WORDS: usize = 20;
const DONOR_WORDS: usize = 6;
const BANK: usize = 15 + ROWS * (ROW_WORDS + DONOR_WORDS);
const WORDS: usize = HEADER + 2 * BANK;

pub type Result<T> = std::result::Result<T, Error>;

/// Install ledger descriptor fork handling before application threads can fork. This opens no
/// files and initializes no CUDA state. The first call (including implicit first `open`) must
/// not race a fork already in progress. Subsequent concurrent open/close/fork is supported.
pub fn initialize() -> Result<()> {
    os::initialize()
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ledger I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("incompatible ledger: {0}")]
    Incompatible(&'static str),
    #[error("invalid ledger operation: {0}")]
    Invalid(&'static str),
    #[error("ledger capacity exhausted: {0}")]
    Full(&'static str),
    #[error("inherited ledger handle cannot be used after fork")]
    Inherited,
    #[error("stale ledger row or round incarnation")]
    Stale,
    #[error("ledger generation overflow")]
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Gpu,
    Mig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub uuid: [u8; 16],
    pub kind: DeviceKind,
}
impl Device {
    /// Caller supplies the actual CUDA GPU/MIG UUID; this metadata module never discovers it.
    pub fn parse(value: &str) -> Result<Self> {
        let (kind, value) = if let Some(uuid) = value.strip_prefix("GPU-") {
            (DeviceKind::Gpu, uuid)
        } else if let Some(uuid) = value.strip_prefix("MIG-") {
            (DeviceKind::Mig, uuid)
        } else {
            return Err(Error::Invalid("expected GPU-UUID or MIG-UUID"));
        };
        Ok(Self {
            kind,
            uuid: uuid(value)?,
        })
    }
    fn name(&self) -> String {
        let mut value = if self.kind == DeviceKind::Gpu {
            "gpu-"
        } else {
            "mig-"
        }
        .to_owned();
        for byte in self.uuid {
            value.push_str(&format!("{byte:02x}"));
        }
        value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub boot: [u8; 16],
    pub uid: u32,
    pub user_namespace: u64,
    pub pid_namespace: u64,
    pub mount_namespace: u64,
}
impl Scope {
    pub fn current() -> Result<Self> {
        Ok(Self {
            boot: uuid(fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim())?,
            uid: os::uid(),
            user_namespace: fs::metadata("/proc/self/ns/user")?.ino(),
            pid_namespace: fs::metadata("/proc/self/ns/pid")?.ino(),
            mount_namespace: fs::metadata("/proc/self/ns/mnt")?.ino(),
        })
    }
}

fn uuid(value: &str) -> Result<[u8; 16]> {
    let bytes: Vec<_> = value.bytes().filter(|&byte| byte != b'-').collect();
    if bytes.len() != 32 {
        return Err(Error::Invalid("UUID must contain 32 hex digits"));
    }
    let mut out = [0; 16];
    for (at, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hex = |byte: u8| {
            (byte as char)
                .to_digit(16)
                .ok_or(Error::Invalid("invalid UUID hex digit"))
        };
        out[at] = (hex(pair[0])? * 16 + hex(pair[1])?) as u8;
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowId {
    pub slot: usize,
    pub incarnation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sharing {
    Shared,
    Dedicated,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bytes {
    pub live: u64,
    pub idle: u64,
    pub pending: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Physical {
    pub weights: Bytes,
    /// Nonpool working segments only: graph backing is counted separately below.
    pub working: Bytes,
    pub graph_pools: Bytes,
    pub caches: Bytes,
}
impl Physical {
    fn classes(self) -> [Bytes; 4] {
        [self.weights, self.working, self.graph_pools, self.caches]
    }
    pub fn total(self) -> Result<u64> {
        self.classes()
            .into_iter()
            .flat_map(|b| [b.live, b.idle, b.pending])
            .try_fold(0u64, |sum, n| sum.checked_add(n).ok_or(Error::Overflow))
    }
    pub fn idle(self) -> Result<u64> {
        self.classes().into_iter().try_fold(0u64, |sum, b| {
            sum.checked_add(b.idle).ok_or(Error::Overflow)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: RowId,
    pub pid: u32,
    pub start_ticks: u64,
    pub sharing: Sharing,
    pub priority: i32,
    pub last_use: u64,
    pub want: u64,
    pub physical: Physical,
    /// Subset of physically idle holdings, never an additional charge or promised release.
    pub reclaimable: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub generation: u64,
    pub total: u64,
    pub driver_used: u64,
    pub observed_at: u64,
    pub reserve: Option<u64>,
    pub attributable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Waiting,
    Pending,
    Complete,
    Refused,
    OwnerGone,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Donation {
    pub owner: RowId,
    pub requested: u64,
    pub held_at_ask: u64,
    pub completed: u64,
    pub pending: u64,
    pub answer: Answer,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub generation: u64,
    pub sample_generation: u64,
    pub requester: RowId,
    pub requested: u64,
    pub aborted: bool,
    pub donations: Vec<Donation>,
}
impl Round {
    pub fn finished(&self) -> bool {
        self.aborted
            || self.donations.iter().all(|d| {
                matches!(
                    d.answer,
                    Answer::Complete | Answer::Refused | Answer::OwnerGone
                )
            })
    }
    pub fn completed(&self) -> Result<u64> {
        self.donations.iter().try_fold(0u64, |sum, d| {
            sum.checked_add(d.completed).ok_or(Error::Overflow)
        })
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub generation: u64,
    pub backing_device: u64,
    pub backing_inode: u64,
    pub scope: Scope,
    pub rows: Vec<Row>,
    pub sample: Option<Sample>,
    pub round: Option<Round>,
}

struct Data {
    next_incarnation: u64,
    sample: Option<Sample>,
    round: Option<Round>,
    rows: [Option<Row>; ROWS],
}
impl Default for Data {
    fn default() -> Self {
        Self {
            next_incarnation: 1,
            sample: None,
            round: None,
            rows: [None; ROWS],
        }
    }
}

struct Connection {
    path: PathBuf,
    file: Arc<os::Descriptor>,
    map: os::Mapping,
    serial: Mutex<()>,
    pid: u32,
    scope: Scope,
    backing: (u64, u64),
}

#[derive(Clone)]
pub struct Ledger(Arc<Connection>);
pub struct Participant {
    ledger: Ledger,
    row: RowId,
    _lock: os::Locked,
}

impl Ledger {
    /// Open one cooperation domain. First initialization must not race fork; call `initialize`
    /// before starting concurrent fork producers. Direct syscall fork/clone bypassing libc's
    /// registered handlers is outside this checkpoint's supported inheritance contract.
    pub fn open(directory: &Path, device: Device) -> Result<Self> {
        let scope = Scope::current()?;
        let directory = directory.canonicalize()?;
        let boot = scope
            .boot
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = directory.join(format!("varena-v3-{}-{boot}-{}", scope.uid, device.name()));
        let file = os::Descriptor::open(&path)?;
        let _publication = file.lock(0, true)?.expect("blocking lock");
        let (dev, ino, size) = file.identity()?;
        if size == 0 {
            file.resize(WORDS * 8)?;
        } else if size != (WORDS * 8) as u64 {
            return Err(Error::Incompatible("layout length"));
        }
        let halves = |bytes: [u8; 16]| {
            [
                u64::from_le_bytes(bytes[..8].try_into().unwrap()),
                u64::from_le_bytes(bytes[8..].try_into().unwrap()),
            ]
        };
        let gpu = halves(device.uuid);
        let boot = halves(scope.boot);
        let expected = [
            MAGIC,
            VERSION,
            WORDS as u64,
            ROWS as u64,
            if device.kind == DeviceKind::Gpu { 1 } else { 2 },
            gpu[0],
            gpu[1],
            boot[0],
            boot[1],
            scope.uid as u64,
            scope.user_namespace,
            scope.pid_namespace,
            scope.mount_namespace,
            u64::from_ne_bytes(*b"abcdefgh"),
            0,
        ];
        let existing = file.header(expected.len())?;
        if existing[0] != 0 && existing.as_slice() != expected {
            return Err(Error::Incompatible(
                "device, boot, UID, namespace or layout identity",
            ));
        }
        // Also allocate correctly sized files: an earlier ENOSPC after ftruncate can leave
        // sparse backing. Reject an incompatible nonzero header before changing allocation.
        file.allocate(WORDS * 8)?;
        let map = os::Mapping::new(&file, WORDS)?;
        if map.load(0, Ordering::Acquire) == 0 {
            // A writer killed during first initialization leaves magic zero; no participant
            // can have registered before the completed header becomes visible.
            for word in HEADER..WORDS {
                map.store(word, 0, Ordering::Relaxed);
            }
            let empty = Data::default().encode();
            for (at, value) in empty.into_iter().enumerate() {
                map.store(HEADER + at, value, Ordering::Relaxed);
            }
            for (at, value) in expected.into_iter().enumerate().skip(1) {
                map.store(at, value, Ordering::Relaxed);
            }
            map.store(SELECTOR, 2, Ordering::Release); // generation 1, bank 0
            map.store(0, MAGIC, Ordering::Release);
        } else {
            for (at, value) in expected.into_iter().enumerate() {
                if map.load(at, Ordering::Acquire) != value {
                    return Err(Error::Incompatible(
                        "device, boot, UID, namespace or layout identity",
                    ));
                }
            }
        }
        drop(_publication);
        Ok(Self(Arc::new(Connection {
            path,
            file,
            map,
            serial: Mutex::new(()),
            pid: std::process::id(),
            scope,
            backing: (dev, ino),
        })))
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    fn check(&self) -> Result<()> {
        if self.0.pid != std::process::id() {
            return Err(Error::Inherited);
        }
        if Scope::current()? != self.0.scope {
            return Err(Error::Incompatible("process changed cooperation namespace"));
        }
        let metadata = fs::symlink_metadata(&self.0.path)?;
        if metadata.file_type().is_symlink() || (metadata.dev(), metadata.ino()) != self.0.backing {
            return Err(Error::Incompatible("backing inode changed"));
        }
        Ok(())
    }

    fn transaction<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut Data, u64) -> Result<T>,
    ) -> Result<T> {
        // Reject inherited handles BEFORE any inherited local mutex can be touched.
        self.check()?;
        let _serial = self
            .0
            .serial
            .lock()
            .map_err(|_| Error::Invalid("poisoned connection mutex"))?;
        let _publication = self.0.file.lock(0, true)?.expect("blocking lock");
        let selected = self.0.map.load(SELECTOR, Ordering::Acquire);
        let generation = selected >> 1;
        if generation == 0 {
            return Err(Error::Incompatible("uncommitted table"));
        }
        let bank = (selected & 1) as usize;
        let words = std::array::from_fn(|at| {
            self.0
                .map
                .load(HEADER + bank * BANK + at, Ordering::Relaxed)
        });
        let mut data = Data::decode(words)?;
        let result = operation(&mut data, generation)?;
        if write {
            let next = generation
                .checked_add(1)
                .filter(|next| *next <= u64::MAX >> 1)
                .ok_or(Error::Overflow)?;
            let other = 1 - bank;
            for (at, value) in data.encode().into_iter().enumerate() {
                self.0
                    .map
                    .store(HEADER + other * BANK + at, value, Ordering::Relaxed);
            }
            // Only this release store publishes the inactive bank. A killed writer leaves
            // the old bank committed; the next writer overwrites the entire other bank.
            self.0
                .map
                .store(SELECTOR, (next << 1) | other as u64, Ordering::Release);
        }
        Ok(result)
    }

    /// Last committed metadata, not a guarantee that every recorded owner is still alive.
    /// `reap_dead` and `ask` perform exact row-lock/birth checks; neither credits GPU capacity.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.transaction(false, |data, generation| {
            Ok(Snapshot {
                generation,
                backing_device: self.0.backing.0,
                backing_inode: self.0.backing.1,
                scope: self.0.scope,
                rows: data.rows.iter().flatten().copied().collect(),
                sample: data.sample,
                round: data.round.clone(),
            })
        })
    }

    /// Reap only registrations whose independent row lock is actually available. This does
    /// not change the driver sample or credit the old row's holdings as returned capacity.
    pub fn reap_dead(&self) -> Result<Vec<Row>> {
        self.transaction(true, |data, _| self.reap(data))
    }
    fn reap(&self, data: &mut Data) -> Result<Vec<Row>> {
        let mut reaped = Vec::new();
        for slot in 0..ROWS {
            let Some(row) = data.rows[slot] else { continue };
            if let Some(_claim) = self.0.file.lock((slot + 1) as i64, false)? {
                data.rows[slot] = None;
                if let Some(round) = &mut data.round {
                    if round.requester == row.id {
                        round.aborted = true;
                    }
                    for donation in &mut round.donations {
                        if donation.owner == row.id {
                            donation.answer = Answer::OwnerGone;
                            donation.pending = 0;
                        }
                    }
                }
                reaped.push(row);
            } else if birth(row.pid)?.is_none_or(|start| start != row.start_ticks) {
                // A raw syscall fork/clone can bypass libc atfork handlers. Its inherited
                // lock is not proof this recorded process is alive; fail explicitly.
                return Err(Error::Incompatible(
                    "ended owner still has an inherited row lock",
                ));
            }
        }
        Ok(reaped)
    }

    pub fn register(&self, sharing: Sharing, priority: i32) -> Result<Participant> {
        let pid = std::process::id();
        let start_ticks = birth(pid)?.ok_or(Error::Invalid("current process has no birth"))?;
        let (id, lock) = self.transaction(true, |data, _| {
            self.reap(data)?;
            if data
                .rows
                .iter()
                .flatten()
                .any(|row| row.pid == pid && row.start_ticks == start_ticks)
            {
                return Err(Error::Invalid("one participant per process/device"));
            }
            let row_file = os::Descriptor::open(&self.0.path)?;
            let mut found = None;
            for slot in 0..ROWS {
                if data.rows[slot].is_none()
                    && let Some(lock) = row_file.lock((slot + 1) as i64, false)?
                {
                    found = Some((slot, lock));
                    break;
                }
            }
            // A departing owner clears its row before its FD closes. Skip that still-held
            // byte instead of mistaking the short teardown interval for corrupt metadata.
            let (slot, lock) = found.ok_or(Error::Full("participant rows"))?;
            let incarnation = data.next_incarnation;
            data.next_incarnation = incarnation.checked_add(1).ok_or(Error::Overflow)?;
            let id = RowId { slot, incarnation };
            data.rows[slot] = Some(Row {
                id,
                pid,
                start_ticks,
                sharing,
                priority,
                last_use: 0,
                want: 0,
                physical: Physical::default(),
                reclaimable: 0,
            });
            Ok((id, lock))
        })?;
        Ok(Participant {
            ledger: self.clone(),
            row: id,
            _lock: lock,
        })
    }
}

impl Participant {
    pub fn id(&self) -> RowId {
        self.row
    }
    fn own<'a>(&self, data: &'a mut Data) -> Result<&'a mut Row> {
        let row = data
            .rows
            .get_mut(self.row.slot)
            .and_then(Option::as_mut)
            .ok_or(Error::Stale)?;
        if row.id != self.row || row.pid != std::process::id() {
            return Err(Error::Stale);
        }
        Ok(row)
    }
    pub fn publish(&self, physical: Physical, reclaimable: u64, last_use: u64) -> Result<()> {
        physical.total()?;
        if reclaimable > physical.idle()? {
            return Err(Error::Invalid(
                "reclaimable bytes are a subset of idle holdings",
            ));
        }
        self.ledger.transaction(true, |data, _| {
            let row = self.own(data)?;
            row.physical = physical;
            row.reclaimable = reclaimable;
            row.last_use = last_use;
            Ok(())
        })
    }
    pub fn configure(&self, sharing: Sharing, priority: i32) -> Result<()> {
        self.ledger.transaction(true, |data, _| {
            let row = self.own(data)?;
            row.sharing = sharing;
            row.priority = priority;
            Ok(())
        })
    }
    /// Record an actual caller observation. This does not manufacture a driver reading.
    pub fn sample(
        &self,
        total: u64,
        driver_used: u64,
        observed_at: u64,
        reserve: Option<u64>,
        attributable: bool,
    ) -> Result<Sample> {
        if driver_used > total || reserve.is_some_and(|value| value > total) {
            return Err(Error::Invalid("inconsistent capacity observation"));
        }
        self.ledger.transaction(true, |data, generation| {
            self.own(data)?;
            let sample = Sample {
                generation: generation + 1,
                total,
                driver_used,
                observed_at,
                reserve,
                attributable,
            };
            data.sample = Some(sample);
            Ok(sample)
        })
    }
    /// Join an unfinished round, or retain the already answered round for the same sample.
    /// Donor requests are advisory; completed acknowledgements still require a driver retry.
    pub fn ask(&self, sample_generation: u64, wanted: u64) -> Result<Round> {
        self.ledger.transaction(true, |data, generation| {
            self.ledger.reap(data)?;
            let mine = *self.own(data)?;
            self.own(data)?.want = wanted;
            if let Some(round) = &data.round
                && (!round.finished() || round.sample_generation == sample_generation)
            {
                return Ok(round.clone());
            }
            if data
                .sample
                .is_none_or(|sample| sample.generation != sample_generation)
            {
                return Err(Error::Stale);
            }
            let mut peers: Vec<_> = data
                .rows
                .iter()
                .flatten()
                .filter(|row| {
                    row.id != mine.id && row.sharing == Sharing::Shared && row.reclaimable != 0
                })
                .copied()
                .collect();
            peers.sort_by_key(|row| (row.priority, row.last_use, row.id.slot));
            let mut remaining = wanted;
            let mut donations = Vec::new();
            for peer in peers {
                if remaining == 0 {
                    break;
                }
                let requested = remaining.min(peer.reclaimable);
                remaining -= requested;
                donations.push(Donation {
                    owner: peer.id,
                    requested,
                    held_at_ask: peer.physical.total()?,
                    completed: 0,
                    pending: 0,
                    answer: Answer::Waiting,
                });
            }
            let round = Round {
                generation: generation + 1,
                sample_generation,
                requester: mine.id,
                requested: wanted,
                aborted: false,
                donations,
            };
            data.round = Some(round.clone());
            Ok(round)
        })
    }
    /// `completed` is cumulative actual physical release reported by this owner, not requested
    /// bytes. Pending release is never included by Round::completed(). Duplicate acks are inert.
    pub fn acknowledge(
        &self,
        round_id: u64,
        completed: u64,
        pending: u64,
        answer: Answer,
    ) -> Result<()> {
        if matches!(answer, Answer::Waiting | Answer::OwnerGone)
            || (answer != Answer::Pending && pending != 0)
        {
            return Err(Error::Invalid("invalid donor acknowledgement"));
        }
        self.ledger.transaction(true, |data, _| {
            self.own(data)?;
            let round = data
                .round
                .as_mut()
                .filter(|round| round.generation == round_id && !round.aborted)
                .ok_or(Error::Stale)?;
            let donor = round
                .donations
                .iter_mut()
                .find(|donor| donor.owner == self.row)
                .ok_or(Error::Stale)?;
            if completed < donor.completed
                || completed > donor.held_at_ask
                || pending > donor.held_at_ask.saturating_sub(completed)
            {
                return Err(Error::Invalid(
                    "acknowledgement is inconsistent with its owner snapshot",
                ));
            }
            if matches!(donor.answer, Answer::Complete | Answer::Refused) {
                return if (completed, pending, answer)
                    == (donor.completed, donor.pending, donor.answer)
                {
                    Ok(())
                } else {
                    Err(Error::Stale)
                };
            }
            donor.completed = completed;
            donor.pending = pending;
            donor.answer = answer;
            Ok(())
        })
    }
}
impl Drop for Participant {
    fn drop(&mut self) {
        if self.ledger.0.pid != std::process::id() {
            return;
        }
        let _ = self.ledger.transaction(true, |data, _| {
            self.own(data)?;
            data.rows[self.row.slot] = None;
            if let Some(round) = &mut data.round {
                if round.requester == self.row {
                    round.aborted = true;
                }
                for donor in &mut round.donations {
                    if donor.owner == self.row {
                        donor.answer = Answer::OwnerGone;
                        donor.pending = 0;
                    }
                }
            }
            Ok(())
        });
    }
}

fn birth(pid: u32) -> Result<Option<u64>> {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let rest = text
        .rsplit_once(") ")
        .ok_or(Error::Invalid("malformed process stat"))?
        .1;
    let fields: Vec<_> = rest.split_whitespace().collect();
    if matches!(fields.first().copied(), Some("Z" | "X")) {
        return Ok(None);
    }
    let start = fields
        .get(19)
        .ok_or(Error::Invalid("truncated process stat"))?
        .parse()
        .map_err(|_| Error::Invalid("invalid process birth"))?;
    Ok(Some(start))
}

impl Data {
    fn encode(&self) -> [u64; BANK] {
        let mut words = [0; BANK];
        words[0] = self.next_incarnation;
        if let Some(sample) = self.sample {
            words[1..8].copy_from_slice(&[
                sample.generation,
                sample.total,
                sample.driver_used,
                sample.observed_at,
                u64::from(sample.reserve.is_some()),
                sample.reserve.unwrap_or(0),
                u64::from(sample.attributable),
            ]);
        }
        if let Some(round) = &self.round {
            words[8..15].copy_from_slice(&[
                round.generation,
                round.sample_generation,
                round.requester.slot as u64,
                round.requester.incarnation,
                round.requested,
                u64::from(round.aborted),
                round.donations.len() as u64,
            ]);
            for (index, donor) in round.donations.iter().enumerate() {
                let offset = 15 + ROWS * ROW_WORDS + index * DONOR_WORDS;
                // Slot and state fit together; incarnation remains independent and never wraps.
                words[offset..offset + DONOR_WORDS].copy_from_slice(&[
                    donor.owner.incarnation,
                    donor.owner.slot as u64 | ((answer(donor.answer) as u64) << 8),
                    donor.requested,
                    donor.held_at_ask,
                    donor.completed,
                    donor.pending,
                ]);
            }
        }
        for (slot, row) in self.rows.iter().enumerate() {
            let Some(row) = row else { continue };
            let offset = 15 + slot * ROW_WORDS;
            words[offset..offset + 7].copy_from_slice(&[
                row.id.incarnation,
                row.pid as u64,
                row.start_ticks,
                if row.sharing == Sharing::Shared { 1 } else { 2 },
                row.priority as i64 as u64,
                row.last_use,
                row.want,
            ]);
            for (index, bytes) in row.physical.classes().into_iter().enumerate() {
                words[offset + 7 + index * 3..offset + 10 + index * 3].copy_from_slice(&[
                    bytes.live,
                    bytes.idle,
                    bytes.pending,
                ]);
            }
            words[offset + 19] = row.reclaimable;
        }
        words
    }
    fn decode(words: [u64; BANK]) -> Result<Self> {
        let mut result = Self {
            next_incarnation: words[0],
            ..Self::default()
        };
        if result.next_incarnation == 0 {
            return Err(Error::Incompatible("row generation"));
        }
        if words[1] != 0 {
            result.sample = Some(Sample {
                generation: words[1],
                total: words[2],
                driver_used: words[3],
                observed_at: words[4],
                reserve: (words[5] != 0).then_some(words[6]),
                attributable: words[7] != 0,
            });
        }
        for slot in 0..ROWS {
            let at = 15 + slot * ROW_WORDS;
            if words[at] == 0 {
                continue;
            }
            let class = |index: usize| Bytes {
                live: words[at + 7 + index * 3],
                idle: words[at + 8 + index * 3],
                pending: words[at + 9 + index * 3],
            };
            let sharing = match words[at + 3] {
                1 => Sharing::Shared,
                2 => Sharing::Dedicated,
                _ => return Err(Error::Incompatible("sharing mode")),
            };
            let row = Row {
                id: RowId {
                    slot,
                    incarnation: words[at],
                },
                pid: words[at + 1]
                    .try_into()
                    .map_err(|_| Error::Incompatible("PID"))?,
                start_ticks: words[at + 2],
                sharing,
                priority: words[at + 4] as i64 as i32,
                last_use: words[at + 5],
                want: words[at + 6],
                physical: Physical {
                    weights: class(0),
                    working: class(1),
                    graph_pools: class(2),
                    caches: class(3),
                },
                reclaimable: words[at + 19],
            };
            if row.reclaimable > row.physical.idle()? {
                return Err(Error::Incompatible("physical accounting"));
            }
            row.physical.total()?;
            result.rows[slot] = Some(row);
        }
        if words[8] != 0 {
            let count: usize = words[14]
                .try_into()
                .map_err(|_| Error::Incompatible("donor count"))?;
            if count > ROWS || words[10] >= ROWS as u64 {
                return Err(Error::Incompatible("round rows"));
            }
            let mut donations = Vec::new();
            for index in 0..count {
                let at = 15 + ROWS * ROW_WORDS + index * DONOR_WORDS;
                let slot = (words[at + 1] & 255) as usize;
                if slot >= ROWS {
                    return Err(Error::Incompatible("donor slot"));
                }
                donations.push(Donation {
                    owner: RowId {
                        slot,
                        incarnation: words[at],
                    },
                    answer: decode_answer(words[at + 1] >> 8)?,
                    requested: words[at + 2],
                    held_at_ask: words[at + 3],
                    completed: words[at + 4],
                    pending: words[at + 5],
                });
            }
            result.round = Some(Round {
                generation: words[8],
                sample_generation: words[9],
                requester: RowId {
                    slot: words[10] as usize,
                    incarnation: words[11],
                },
                requested: words[12],
                aborted: words[13] != 0,
                donations,
            });
        }
        Ok(result)
    }
}
fn answer(answer: Answer) -> u8 {
    match answer {
        Answer::Waiting => 0,
        Answer::Pending => 1,
        Answer::Complete => 2,
        Answer::Refused => 3,
        Answer::OwnerGone => 4,
    }
}
fn decode_answer(value: u64) -> Result<Answer> {
    match value {
        0 => Ok(Answer::Waiting),
        1 => Ok(Answer::Pending),
        2 => Ok(Answer::Complete),
        3 => Ok(Answer::Refused),
        4 => Ok(Answer::OwnerGone),
        _ => Err(Error::Incompatible("donor answer")),
    }
}
