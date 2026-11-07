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
use varena_ledger::{Donor, Recovery, Row, Table};

/// The room a process needs to come up: its CUDA context and first stream (xs-112: 2 to 3 GB).
const BRING_UP_BYTES: usize = 3 << 30;
/// How long an ask waits on peers that have stopped answering.
const PEER_WAIT: Duration = Duration::from_secs(2);
/// Less is not worth a room ask: demand asks for it.
const SMALLEST_ROOM: usize = 256 << 20;
/// A request's room asks: each one that helps is followed by another, up to this many.
const ROOM_ASKS: usize = 32;
/// How long a donor waits on its own cache owners, which may need a GIL someone holds.
const OWNER_WAIT: Duration = Duration::from_secs(1);
/// A foreign shortage lasting this many looks is past what gives can mend.
const PRESSED_TICKS: u32 = 3;
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
    // One ask for the context and the first stream together: Torch creates that stream, where a
    // refusal reaches nobody who could give room.
    let mut last = false;
    loop {
        let started = working::initialize(device);
        let short = match &started {
            Err(Error::Cuda { code: 2, .. }) => true,
            Ok(()) => working::peer::memory_info(device).is_ok_and(|(free, _)| free < BRING_UP_BYTES),
            Err(_) => false,
        };
        if !short || last {
            break started;
        }
        last = recover(device, BRING_UP_BYTES, Instant::now() + PEER_WAIT) == Recovery::Final;
    }?;
    let display = Nvml::init().ok().and_then(|nvml| nvml_device(&nvml, &uuid)?.is_display_active().ok());
    if display == Some(true) {
        crate::gpu(device)?.drives_display(true);
        eprintln!("varena: GPU {device} drives a display: a sixteenth of it, at least 512 MiB, stays free past every program's use");
    }
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

/// A request begins needing `need` bytes here. What this process lacks of it is asked for on a
/// thread of its own while the request runs, never on its fill path: idle peers give back what
/// they hold idle, and when that is not enough the ledger names a process in no request and
/// without priority, which its supervisor ends, since only an exit returns a context. At 6 GiB
/// four idle executors held a third of the card (runs 40927, 40978); an idle SDXL's 5.3 GiB asked
/// for on demand cost Qwen 1-1.7 s of step 1.
pub(crate) fn room_for(gpu: &'static Device, need: usize) {
    let device = gpu.ordinal();
    let Some(service) = service(device) else { return };
    let short = move || -> Result<usize> {
        crate::gate(device)?.not_capturing()?;
        let (free, total) = working::peer::memory_info(device)?;
        Ok(gpu.short_of(need, free, total))
    };
    let spawned = std::thread::Builder::new().name(format!("varena-room-{device}")).spawn(move || {
        for _ in 0..ROOM_ASKS {
            let Ok(bytes) = short() else { return };
            if bytes < SMALLEST_ROOM || !service.table.running(service.row) {
                return;
            }
            if service.table.ask_room(service.row, bytes as u64, Instant::now() + PEER_WAIT) == Recovery::Final {
                return;
            }
        }
    });
    if let Err(error) = spawned {
        eprintln!("varena: GPU {device}'s room comes on demand only: {error}");
    }
}

pub(crate) fn request_boundary(device: i32, begin: bool) {
    if let Some(service) = service(device) {
        if begin { service.table.request_begin(service.row) } else { service.table.request_end(service.row) }
    }
}

fn nvml_device<'a>(nvml: &'a Nvml, b: &[u8; 16]) -> Option<nvml_wrapper::Device<'a>> {
    let name = format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
    nvml.device_by_uuid(format!("GPU-{name}")).or_else(|_| nvml.device_by_uuid(format!("MIG-{name}"))).ok()
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
    /// Ticks in a row the card has been short since foreign use grew.
    pressed: u32,
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
            pressed: 0,
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
    /// A foreign shortage that outlasts every give, with nothing idle left anywhere, asks the
    /// supervisor for an exit: one donor's word is enough.
    fn tick(&mut self, table: &Table, row: Row, idle: u64) -> bool {
        // Right after a give its taker may not have taken it yet: the next look comes as soon.
        let gave = std::mem::take(&mut self.asked);
        if gave {
            table.stop_asking(row);
        }
        if !self.shared {
            return false;
        }
        let sample = crate::gate(self.device).and_then(|gate| {
            gate.not_capturing()?;
            working::peer::memory_info(self.device)
        });
        let Ok((free, total)) = sample else { return false };
        let (free, total) = (free as u64, total as u64);
        let reserve = (total / 16).max(512 << 20);
        // Short only for memory that isn't ours: a process filling its own weights asks for itself.
        let foreign = (total - free).saturating_sub(table.held());
        let grew = foreign > std::mem::replace(&mut self.foreign, foreign);
        self.pressed = if free < reserve && (grew || self.pressed != 0) { self.pressed + 1 } else { 0 };
        if self.pressed > PRESSED_TICKS && table.idle() == 0 {
            table.foreign_short();
        }
        if idle == 0 {
            return false;
        }
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
        want != 0 || gave
    }
}
