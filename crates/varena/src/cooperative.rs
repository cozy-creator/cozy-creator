//! This process's part in sharing a GPU: a row in its ledger, one donor thread, and the sample of
//! foreign pressure. All of it is advisory: a table that cannot open or a peer that dies costs
//! donations, never an allocation.
use crate::{Error, Result, arena_retirement::Observation, cache_owner, capture_counts, cuda, working};
use nvml_wrapper::{Nvml, enums::device::UsedGpuMemory};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use varena_ledger::{Donor, Recovery, Row, Table};

/// What to ask for when a CUDA context itself is refused (xs-112: 2 to 3 GB).
const CONTEXT_BYTES: usize = 3 << 30;
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
    // The row comes first: a context can itself be refused for memory, and then we ask.
    let uuid = cuda::device_uuid(device)?;
    let service = match Table::open(&uuid).map(|table| table.claim().map(|row| Service { table, row })) {
        Ok(Some(service)) => Some(Arc::new(service)),
        Ok(None) => { eprintln!("varena: GPU {device}'s ledger is full; this process shares no memory"); None }
        Err(error) => { eprintln!("varena: GPU {device} has no ledger: {error}"); None }
    };
    if let Some(service) = &service {
        SERVICES.lock().unwrap_or_else(|error| error.into_inner()).insert(device, Arc::clone(service));
    }
    let mut last = false;
    loop {
        match working::initialize(device) {
            Err(Error::Cuda { code: 2, .. }) if !last => {
                last = recover(device, CONTEXT_BYTES, Instant::now() + PEER_WAIT) == Recovery::Final;
            }
            result => break result,
        }
    }?;
    if let Some(service) = service {
        let mut donor = Native::new(device, shared, uuid)?;
        std::thread::Builder::new().name(format!("varena-owner-{device}")).spawn(move || {
            let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.table.serve(service.row, &mut donor)));
            // Only reached on a panic. The row goes, so nobody waits on this process.
            eprintln!("varena: GPU {device}'s donor ended: {served:?}");
            service.table.leave(service.row);
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

pub(crate) fn request_boundary(device: i32, begin: bool) {
    if let Some(service) = service(device) {
        if begin { service.table.request_begin(service.row) } else { service.table.request_end(service.row) }
    }
}

struct Native {
    device: i32,
    shared: bool,
    room: Arc<crate::Room>,
    retained: Vec<Arc<Observation>>,
    idle: u64,
    asked: bool,
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
            room: crate::room(device)?,
            retained: Vec::new(),
            idle: 0,
            asked: false,
            nvml: if host_pids { Nvml::init().ok() } else { None },
            uuid,
            previous: None,
        })
    }

    fn count_idle(&mut self) -> Result<u64> {
        let mut idle = working::peer::idle_bytes(self.device)?;
        for arena in crate::retain_arenas(self.device, &mut self.retained)?.arenas {
            for entry in arena.lock()?.entries.values() {
                let in_use = entry.uses.pins() != 0 || entry.uses.state() == crate::Contents::Loading
                    || entry.uses.unjoined.load(Ordering::Acquire);
                if entry.handle.is_some() && !in_use {
                    idle += entry.bytes as u64;
                }
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
        let b = self.uuid;
        let name = format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
        let device = nvml.device_by_uuid(format!("GPU-{name}")).or_else(|_| nvml.device_by_uuid(format!("MIG-{name}"))).ok()?;
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
        let held = self.room.pages.load(Ordering::Acquire) + self.room.working.load(Ordering::Acquire);
        // A momentary refusal keeps the last count: zero would read as nothing left to ask for.
        self.idle = self.count_idle().unwrap_or(self.idle);
        (held as u64, self.idle)
    }

    fn trim(&mut self, bytes: u64) -> u64 {
        let wanted = bytes as usize;
        let released = || self.room.released.load(Ordering::Acquire);
        let since = released();
        // A capture or a replay refuses this: the asker comes back while idle memory remains.
        let _ = crate::reclaim_completed(self.device, wanted, &mut self.retained);
        if released() - since < bytes && cache_owner::requestable(self.device).is_ok_and(|owners| owners != 0) {
            // Only its owners can shrink a cache. Their worker runs them, so a GIL never blocks this thread.
            let key = cache_owner::next_generation().map(|generation| cache_owner::RequestKey { cause: cache_owner::Cause::Allocation, generation });
            if let Ok(Ok(Some(request))) = key.map(|key| cache_owner::request_shrink(self.device, key, wanted)) {
                let _ = request.wait_allocation_release(since, Instant::now() + OWNER_WAIT);
            }
        }
        released() - since
    }

    /// Design §4.6: `free < H` gives back up to `2H - free`, wherever a foreign program or this
    /// card's own fill put it, and speculation regrows at a pass boundary only once `2H` is free.
    fn tick(&mut self, table: &Table, row: Row, idle: u64) {
        if std::mem::take(&mut self.asked) {
            table.stop_asking(row);
        }
        if !self.shared || idle == 0 {
            return;
        }
        let sample = capture_counts().and_then(|captures| {
            crate::ensure_recovery_allowed(&captures, self.device)?;
            working::peer::memory_info(self.device)
        });
        let Ok((free, total)) = sample else { return };
        let (free, total) = (free as u64, total as u64);
        let reserve = (total / 16).max(512 << 20);
        let want = if self.new_foreign_process(table) {
            total
        } else if free < reserve {
            2 * reserve - free
        } else {
            0
        };
        let _ = working::peer::pressure(self.device, want != 0, want == 0 && free >= 2 * reserve);
        if want != 0 {
            table.ask(row, want);
            self.asked = true;
        }
    }
}
