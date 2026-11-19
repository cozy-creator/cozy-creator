//! This process's part in sharing a GPU: a row in its ledger, one donor thread, and the sample of
//! foreign pressure. All of it is advisory: a table that cannot open or a peer that dies costs
//! donations, never an allocation.
use crate::{Device, Error, Result, cache_owner, cuda, working};
use nvml_wrapper::{Nvml, enums::device::UsedGpuMemory};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use varena_ledger::{Donor, Look, Recovery, Row, Table};

/// How long an ask waits on peers that have stopped answering.
const PEER_WAIT: Duration = Duration::from_secs(2);
/// How long a donor waits on its own cache owners, which may need a GIL someone holds.
const OWNER_WAIT: Duration = Duration::from_secs(1);
/// A foreign process crossing this is a program starting: all idle memory goes back to it.
const NEW_PROCESS: u64 = 256 << 20;
/// Linux UAPI nsfs.h: only in the initial PID namespace are NVML's pids ours.
const PID_NS_INIT_INO: u64 = 0xefff_fffc;

struct Service {
    table: Table,
    row: Row,
}

static SERVICES: Mutex<BTreeMap<i32, Arc<Service>>> = Mutex::new(BTreeMap::new());
static JOINING: Mutex<()> = Mutex::new(());

/// A claimed row until its donor stops serving it. Dropped, the row goes, so no peer waits on it.
struct Serving(i32, Arc<Service>);

impl Drop for Serving {
    fn drop(&mut self) {
        SERVICES.lock().unwrap_or_else(|error| error.into_inner()).remove(&self.0);
        self.1.table.leave(self.1.row);
    }
}

fn service(device: i32) -> Option<Arc<Service>> {
    SERVICES.lock().unwrap_or_else(|error| error.into_inner()).get(&device).cloned()
}

/// Take this process's row on `device`, open its context and start its donor. Idempotent: the
/// first caller's `shared` holds. Dedicated (`shared` false) only means not yielding to foreign
/// programs: every participant donates to every other in both modes.
pub(crate) fn join(device: i32, shared: bool) -> Result<()> {
    let _joining = JOINING.lock().unwrap_or_else(|error| error.into_inner());
    if working::cooperation_bound(device)? {
        return Ok(());
    }
    // The row comes first: coming up can itself be refused for memory, and then we ask.
    let uuid = cuda::device_uuid(device)?;
    let serving = match Table::open(&uuid).map(|table| table.claim().map(|row| Service { table, row })) {
        Ok(Some(service)) => {
            let service = Arc::new(service);
            SERVICES.lock().unwrap_or_else(|error| error.into_inner()).insert(device, Arc::clone(&service));
            Some(Serving(device, service))
        }
        Ok(None) => { eprintln!("varena: GPU {device}'s ledger is full; this process shares no memory"); None }
        Err(error) => { eprintln!("varena: GPU {device} has no ledger: {error}"); None }
    };
    // Once the context is up, the driver's room this process is expected to need (`expect`) must be
    // free: Torch's runtime and first stream come next, where a refusal reaches nobody who could give
    // room (run 31653: an idle H3 left under 700 MiB). Short of it, peers are asked for the rest; a
    // refused context asks for what peers can give. The context's cost is measured as it comes up:
    // what was free before it less what is free after. 3 GiB was asked of peers before, on any card.
    let before = card_free(device);
    let mut last = false;
    loop {
        let started = working::initialize(device);
        let short = match &started {
            Err(Error::Cuda { code: 2, .. }) => Some(0),
            Ok(()) => card_free(device).and_then(|free| {
                let context = before.map_or(0, |before| before.saturating_sub(free));
                let room = came_up(device, Expected { uuid, context, set: None });
                (free < room).then(|| room - free)
            }),
            Err(_) => None,
        };
        match short {
            Some(short) if !last => last = recover(device, short, Instant::now() + PEER_WAIT) == Recovery::Final,
            _ => break started,
        }
    }?;
    came_up(device, Expected { uuid, context: 0, set: None });
    // What other programs hold as this process begins: a wait is only ever for what they took since.
    let _ = others_came(device);
    if let Some(serving) = serving {
        let mut donor = Native::new(device, shared, uuid)?;
        std::thread::Builder::new().name(format!("varena-owner-{device}")).spawn(move || {
            let Serving(_, service) = &serving;
            let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.table.serve(service.row, &mut donor)));
            eprintln!("varena: GPU {device}'s donor ended: {served:?}");
        }).map_err(|error| Error::Unsupported(format!("donor thread: {error}")))?;
    }
    Ok(())
}

/// The allocating thread, after local reclaim failed and with no lock held.
pub(crate) fn recover(device: i32, bytes: usize, deadline: Instant) -> Recovery {
    match service(device) {
        Some(service) => service.table.recover(service.row, bytes as u64, deadline),
        None => Recovery::Final,
    }
}

/// Whether this process is in a request on `device`; None with no ledger row to say.
pub(crate) fn in_request(device: i32) -> Option<bool> {
    service(device).map(|service| service.table.running(service.row))
}

pub(crate) fn peers_idle(device: i32) -> bool {
    service(device).is_some_and(|service| service.table.peers_idle(service.row))
}

pub(crate) fn ask_idle_peers(device: i32, bytes: usize) -> bool {
    service(device).is_some_and(|service| service.table.ask_idle_peers(service.row, bytes as u64, Instant::now() + PEER_WAIT))
}

pub(crate) fn request_boundary(device: i32, begin: bool) {
    if let Some(service) = service(device) {
        if begin { service.table.request_begin(service.row) } else { service.table.request_end(service.row) }
    }
    let room = expect(device, !begin);
    if begin && let Ok(gpu) = crate::gpu(device) {
        gpu.driver_room_began(room);
    }
}

/// What a process on each GPU is expected to hold outside its books, as read when it came up.
#[derive(Clone, Copy)]
struct Expected {
    uuid: [u8; 16],
    /// Its context's measured cost (`join`): 0 where NVML could not read the card.
    context: usize,
    /// The model set its latest plan runs (`plan_seen`): what it learns under and reads back.
    set: Option<u64>,
}

static EXPECTED: Mutex<BTreeMap<i32, Expected>> = Mutex::new(BTreeMap::new());

/// This process is up on `device`, as `expected` read it the first time: the driver's room it needs.
fn came_up(device: i32, expected: Expected) -> usize {
    EXPECTED.lock().unwrap_or_else(|error| error.into_inner()).entry(device).or_insert(expected);
    expect(device, false)
}

/// A plan running model set `set` became active on `device`: its learned figure now applies.
pub(crate) fn plan_seen(device: i32, set: u64) {
    let changed = EXPECTED.lock().unwrap_or_else(|error| error.into_inner()).get_mut(&device)
        .is_some_and(|expected| expected.set.replace(set) != Some(set));
    if changed {
        expect(device, false);
    }
}

/// The driver's room every map leaves on `device` (`Device::driver_room`): the most a process running
/// this model set on this card held outside its books (learned on this machine, kept across boots)
/// less what this process holds there now, measured (by NVML, else read from the card); for a set
/// this card never ran, its context's measured cost. Learned across every model set, a process
/// inherited another's growth. A fixed 800 MiB was left before, 10-13% of an 8 GB card. `learn`: a
/// request ended, and what this process holds outside its books joins what its set learned. Read as
/// the process comes up, as a plan becomes active and at each request's boundaries.
fn expect(device: i32, learn: bool) -> usize {
    let Ok(gpu) = crate::gpu(device) else { return 0 };
    let Some(Expected { uuid, context, set }) = EXPECTED.lock().unwrap_or_else(|error| error.into_inner())
        .get(&device).copied()
    else {
        return 0;
    };
    // Where NVML can't name this process (a container's nested PID namespace), what it holds outside its
    // books is read from the card: its context, and what lies outside its books now past the least it
    // did. Unread, a 4090's room stood at its context's 391 MiB under an 8 GiB cap for every request,
    // and a prompt LM streamed 20% more a token.
    let measured = split(device).map(|(ours, _)| (ours as usize).saturating_sub(gpu.owned()));
    let floor = gpu.outside_floor.load(Ordering::Acquire);
    let grown = if floor == usize::MAX { 0 } else { gpu.outside.load(Ordering::Acquire).saturating_sub(floor) };
    let own = measured.unwrap_or(context + grown);
    if learn && let Some(set) = set {
        let _ = varena_ledger::held_out_of_books(&uuid, set, own as u64);
    }
    let most = set.and_then(|set| varena_ledger::out_of_books(&uuid, set));
    let room = most.map_or(context, |most| (most as usize).saturating_sub(own));
    gpu.expect_driver(room, measured.unwrap_or(0), context, most.map_or(0, |most| most as usize));
    room
}

fn nvml_device<'a>(nvml: &'a Nvml, b: &[u8; 16]) -> Option<nvml_wrapper::Device<'a>> {
    let name = format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
    nvml.device_by_uuid(format!("GPU-{name}")).or_else(|_| nvml.device_by_uuid(format!("MIG-{name}"))).ok()
}

static NVML: std::sync::OnceLock<Option<Nvml>> = std::sync::OnceLock::new();

/// The card's free bytes as NVML reads them, which needs no context of ours. None where it can't.
fn card_free(device: i32) -> Option<usize> {
    let uuid = cuda::device_uuid(device).ok()?;
    let nvml = NVML.get_or_init(|| Nvml::init().ok()).as_ref()?;
    Some(nvml_device(nvml, &uuid)?.memory_info().ok()?.free as usize)
}
/// Each device's least memory other programs held, NVML's, from this process's start.
static OTHERS: Mutex<BTreeMap<i32, u64>> = Mutex::new(BTreeMap::new());

/// The card's memory by process, NVML's: this process's and every other's. None unless NVML lists this
/// process under its own pid (in its own PID namespace, as recent drivers translate a container's too).
fn split(device: i32) -> Option<(u64, u64)> {
    let uuid = cuda::device_uuid(device).ok()?;
    let nvml = NVML.get_or_init(|| Nvml::init().ok()).as_ref()?;
    let gpu = nvml_device(nvml, &uuid)?;
    let mut rows = BTreeMap::<u32, u64>::new();
    for process in gpu.running_compute_processes().ok()?.into_iter().chain(gpu.running_graphics_processes().ok()?) {
        let UsedGpuMemory::Used(bytes) = process.used_gpu_memory else { return None };
        let value = rows.entry(process.pid).or_default();
        *value = (*value).max(bytes); // Compute and graphics can report the same process.
    }
    let me = std::process::id();
    let ours = *rows.get(&me)?;
    Some((ours, rows.iter().filter(|(pid, _)| **pid != me).map(|(_, bytes)| *bytes).sum()))
}

/// What other programs took on `device` since this process began, which a wait may get back. This
/// process's own memory beyond its pages (its context, modules, libraries' workspaces) is outside
/// them too and never comes back: NVML splits the card by process where it lists this one, else only
/// peers' ledger rows are known to be another's (var-019: a capped executor waited 300 s for 155 MB
/// that was its own, run 47309).
pub(crate) fn others_came(device: i32) -> usize {
    if let Some((_, others)) = split(device) {
        let mut floors = OTHERS.lock().unwrap_or_else(|error| error.into_inner());
        let floor = floors.entry(device).or_insert(others);
        *floor = (*floor).min(others);
        return others.saturating_sub(*floor) as usize;
    }
    service(device).map_or(0, |service| service.table.held_elsewhere_bytes(service.row) as usize)
}

/// What the driver keeps of this card: NVML counts it and CUDA's total leaves it out (378 MiB of a
/// 12 GiB 3060). Zero when NVML can't say.
pub(crate) fn driver_reservation(device: i32, cuda_total: usize) -> usize {
    let measured = (|| {
        let uuid = cuda::device_uuid(device).ok()?;
        let nvml = Nvml::init().ok()?;
        let total = nvml_device(&nvml, &uuid)?.memory_info().ok()?.total as usize;
        total.checked_sub(cuda_total)
    })();
    measured.unwrap_or(0)
}

struct Native {
    device: i32,
    shared: bool,
    gpu: &'static Device,
    idle: u64,
    asked: bool,
    /// Ticks in a row the card has been short: each asks twice the last.
    short: u32,
    /// The card's use that no row holds, at the last tick.
    foreign: u64,
    nvml: Option<Nvml>,
    uuid: [u8; 16],
    previous: Option<BTreeMap<u32, u64>>,
}

impl Native {
    fn new(device: i32, shared: bool, uuid: [u8; 16]) -> Result<Self> {
        let host_pids = shared && std::fs::metadata("/proc/self/ns/pid").is_ok_and(|ns| ns.ino() == PID_NS_INIT_INO);
        Ok(Self {
            device,
            shared,
            gpu: crate::gpu(device)?,
            idle: 0,
            asked: false,
            short: 0,
            foreign: 0,
            nvml: if host_pids { Nvml::init().ok() } else { None },
            uuid,
            previous: None,
        })
    }

    fn count_idle(&mut self) -> Result<u64> {
        let mut idle = working::peer::idle_bytes(self.device)?;
        for entry in self.gpu.lock()?.entries.values() {
            let in_use = entry.uses.pins() != 0 || entry.uses.state() == crate::Contents::Loading
                || !self.gpu.joined(entry.uses.unjoined.load(Ordering::Acquire));
            if entry.handle.is_some() && !in_use {
                idle += entry.bytes as u64;
            }
        }
        Ok(idle)
    }

    /// Whether a foreign process just crossed `NEW_PROCESS`. Unknown in a PID namespace.
    fn new_foreign_process(&mut self, table: &Table) -> bool {
        let Some(rows) = self.processes() else {
            self.previous = None;
            return false;
        };
        let peers = table.pids();
        let foreign: BTreeMap<u32, u64> = rows.into_iter().filter(|(pid, _)| !peers.contains(pid)).collect();
        let crossed = self.previous.as_ref().is_some_and(|previous| foreign.iter().any(|(pid, bytes)|
            *bytes >= NEW_PROCESS && previous.get(pid).copied().unwrap_or(0) < NEW_PROCESS));
        self.previous = Some(foreign);
        crossed
    }

    fn processes(&self) -> Option<BTreeMap<u32, u64>> {
        let nvml = self.nvml.as_ref()?;
        let device = nvml_device(nvml, &self.uuid)?;
        let mut rows = BTreeMap::<u32, u64>::new();
        for process in device.running_compute_processes().ok()?.into_iter().chain(device.running_graphics_processes().ok()?) {
            let UsedGpuMemory::Used(bytes) = process.used_gpu_memory else { return None };
            let value = rows.entry(process.pid).or_default();
            *value = (*value).max(bytes); // Compute and graphics can report the same process.
        }
        Some(rows)
    }
}

impl Donor for Native {
    fn holdings(&mut self) -> (u64, u64) {
        let held = self.gpu.owned();
        // A momentary refusal keeps the last count: zero would read as nothing left to ask for.
        self.idle = self.count_idle().unwrap_or(self.idle);
        (held as u64, self.idle)
    }

    fn trim(&mut self, bytes: u64) -> u64 {
        let wanted = bytes as usize;
        let released = || self.gpu.released.load(Ordering::Acquire);
        let since = released();
        // A capture or a replay refuses this: the asker comes back while idle memory remains.
        let _ = crate::reclaim_completed(self.device, wanted);
        if released() - since < bytes && cache_owner::requestable(self.device).is_ok_and(|owners| owners != 0) {
            // Only its owners can shrink a cache. Their worker runs them, so a GIL never blocks this thread.
            let key = cache_owner::next_generation().map(|generation| cache_owner::RequestKey { generation });
            if let Ok(Ok(Some(request))) = key.map(|key| cache_owner::request_shrink(self.device, key, wanted)) {
                let _ = request.wait_release(since, Instant::now() + OWNER_WAIT);
            }
        }
        if released() - since < bytes {
            let _ = crate::reclaim::release_high(self.device, wanted);
        }
        released() - since
    }

    /// Design §4.6: `free < H` gives back up to `2H - free`, wherever a foreign program or this
    /// card's own fill put it, and speculation regrows at a pass boundary only once `2H` is free.
    /// Sampled only on a shared card with idle bytes: the driver tells no one of another program's use.
    fn tick(&mut self, table: &Table, row: Row, idle: u64) -> Look {
        // Right after a give its taker may not have taken it yet: the next look comes as soon.
        let gave = std::mem::take(&mut self.asked);
        if gave {
            table.stop_asking(row);
        }
        if !self.shared || idle == 0 {
            return Look::Nothing;
        }
        let sample = crate::gate(self.device).and_then(|gate| {
            gate.not_capturing()?;
            working::peer::memory_info(self.device)
        });
        let Ok((free, total)) = sample else { return Look::Sample };
        let (free, total) = (free as u64, total as u64);
        let reserve = (total / 16).max(512 << 20);
        // Short only for memory that isn't ours: a process filling its own weights asks for itself.
        let foreign = (total - free).saturating_sub(table.held());
        let grew = foreign > std::mem::replace(&mut self.foreign, foreign);
        let want = if self.new_foreign_process(table) {
            total
        } else if free < reserve && grew {
            // A shortage that outlasts a give is met in a few ticks, not one gigabyte a tick.
            ((2 * reserve - free) << self.short.min(total.leading_zeros())).min(total)
        } else {
            0
        };
        let _ = working::peer::pressure(self.device, want != 0, want == 0 && free >= 2 * reserve);
        self.short = if want != 0 { self.short + 1 } else if gave { self.short } else { 0 };
        if want != 0 {
            table.ask(row, want);
            self.asked = true;
        }
        if want != 0 || gave { Look::Short } else { Look::Sample }
    }
}
