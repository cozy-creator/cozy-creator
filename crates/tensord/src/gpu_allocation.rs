//! GPU allocations, as Slurm's: a root run (a request with no parent) holds a contiguous lane
//! of GPUs from its first GPU step until `Context.release_gpus()` or its executor's exit, and
//! its calls (steps) run only there, one call per GPU at a time. Roots queue by acceptance and
//! take their place only with a GPU step ready (`Claims`): a root still downloading or in CPU
//! start-up claims nothing, so younger ready roots use the GPUs meanwhile. A holder keeps its
//! lane between its steps; only an older root's first ready step takes it from a younger
//! holder, at the holder's next step boundary: the holder starts no new step, its calls in
//! flight and their families finish (nothing is preempted), then its lane ends and it queues
//! again at its own place. A root that released (or yielded) its lane comes back at its place
//! among the waiting ones and never displaces a holder. A call's family may call where it
//! calls: a call waiting on its child shares its GPU with it.
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, Mutex, OnceLock},
};

/// Per GPU, the oldest request waiting for it in this dispatch pass.
pub(crate) type Claims = [Option<u64>];

pub(crate) struct Allocations {
    table: Mutex<Table>,
    /// Dispatch's wake, run once the table is unlocked after every change that can let a waiting
    /// call place: a call slot, lane or root ended, or pressure changed. A pass that found a GPU
    /// busy is always followed by one that sees it free.
    wake: OnceLock<Box<dyn Fn() + Send + Sync>>,
}
struct Table {
    gpus: Vec<Gpu>,
    roots: BTreeMap<u64, Root>,
}
#[derive(Default)]
struct Gpu {
    /// Known external memory demand holds new families until that demand recedes.
    external_pressure: bool,
    /// The root whose lane this GPU is.
    root: Option<u64>,
    /// Calls running there, all of one family.
    calling: usize,
    /// The family of the calls running there, if any, and their root.
    caller: Option<String>,
    running: Option<u64>,
    /// How many of its calls are prefetch prewarms, which end with their owner (`GpuPool::prefetch`).
    hints: usize,
}
struct Root {
    width: usize,
    lane: Option<Range<usize>>,
    released: bool,
    /// It held a lane before: its later steps wait for free GPUs, displacing no holder.
    started: bool,
    /// The blocker its newest reported wait named, until it binds.
    reported: Option<Option<u64>>,
    holders: usize,
}
/// A root's lane, or the request it waits behind and what holds it. `new`: the first answer
/// since it changed.
pub(crate) enum Lane {
    Held { lane: Range<usize>, new: bool },
    Waiting { behind: Option<u64>, blocker: Blocker, new: bool },
}
/// What a waiting root's lane is held by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blocker {
    /// Another request's lane, claim or call (`hint`: a prefetch's prewarm).
    Request { seq: u64, hint: bool },
    /// Machine work of no request: warming, or an idle executor ending.
    Machine,
    /// Memory another process uses on the GPU.
    Pressure,
}
/// What a released root's executor holds: none of its lane, until its next step binds one.
pub(crate) struct Hold {
    table: Arc<Allocations>,
    root: u64,
}
pub(crate) struct Permit {
    table: Arc<Allocations>,
    devices: Range<usize>,
    hint: bool,
}

impl Table {
    fn free(&self, device: usize, root: u64, claims: &Claims) -> bool {
        let gpu = &self.gpus[device];
        !gpu.external_pressure && gpu.root.is_none() && gpu.calling == 0
            && claims.get(device).copied().flatten().is_none_or(|claimer| claimer == root)
    }
    /// `root` gets `device` at the latest at its holder's next step boundary: no other request
    /// waits for it first, and it is free or (for a root's first step) a younger root holds it.
    fn obtainable(&self, device: usize, root: u64, first: bool, claims: &Claims) -> bool {
        !self.gpus[device].external_pressure && self.gpus[device].root.is_none_or(|holder| first && holder > root)
            && claims.get(device).copied().flatten().is_none_or(|claimer| claimer == root)
    }
    /// The oldest request besides `root` that claims, holds or still calls on a GPU of `lane`;
    /// `root`'s own claim from an earlier step this pass hides none of them.
    fn other(&self, lane: Range<usize>, root: u64, claims: &Claims) -> Option<u64> {
        lane.flat_map(|d| [claims.get(d).copied().flatten(), self.gpus[d].root, self.gpus[d].running])
            .flatten().filter(|other| *other != root).min()
    }
    /// What holds `lane` from `root`: the request `other` names, else the machine's own work.
    fn blocker(&self, lane: Range<usize>, root: u64, claims: &Claims) -> Blocker {
        match self.other(lane.clone(), root, claims) {
            Some(seq) => Blocker::Request {
                seq, hint: lane.clone().any(|d| {
                    let gpu = &self.gpus[d];
                    gpu.running == Some(seq) && gpu.hints > 0 && gpu.hints == gpu.calling
                }),
            },
            None if lane.clone().any(|d| self.gpus[d].external_pressure) => Blocker::Pressure,
            None => Blocker::Machine,
        }
    }
    /// No call runs on `root`'s lane.
    fn between_steps(&self, root: u64) -> bool {
        self.gpus.iter().filter(|gpu| gpu.root == Some(root)).all(|gpu| gpu.calling == 0)
    }
    /// An older root's first ready step waits for `root`'s lane: it starts no new step.
    fn yielding(&self, root: u64, claims: &Claims) -> bool {
        self.gpus.iter().zip(claims).any(|(gpu, claim)| {
            gpu.root == Some(root) && claim.is_some_and(|claimer| claimer < root && self.roots.contains_key(&claimer))
        })
    }
    /// A call of `family` continues a step in flight: its family calls on a GPU.
    fn continuing(&self, family: Option<&str>) -> bool {
        family.is_some_and(|family| self.gpus.iter().any(|gpu| gpu.caller.as_deref() == Some(family)))
    }
    fn bind(&mut self, root: u64, lane: Range<usize>) {
        lane.clone().for_each(|d| self.gpus[d].root = Some(root));
        let entry = self.roots.get_mut(&root).expect("a registered root");
        (entry.lane, entry.released, entry.started, entry.reported) = (Some(lane), false, true, None);
    }
}

impl Allocations {
    pub(crate) fn new(width: usize) -> Arc<Self> {
        let gpus = (0..width).map(|_| Gpu::default()).collect();
        Arc::new(Self { table: Mutex::new(Table { gpus, roots: BTreeMap::new() }), wake: OnceLock::new() })
    }
    pub(crate) fn wake_with(&self, wake: impl Fn() + Send + Sync + 'static) {
        let _ = self.wake.set(Box::new(wake));
    }
    fn freed(&self) {
        if let Some(wake) = self.wake.get() { wake() }
    }
    pub(crate) fn pressure(&self, device: usize, blocked: bool) {
        self.table.lock().unwrap().gpus[device].external_pressure = blocked;
        self.freed();
    }
    /// `root` holds an allocation `width` GPUs wide, unbound until its first step, for as long
    /// as the hold lives.
    pub(crate) fn register(self: &Arc<Self>, root: u64, width: usize) -> Hold {
        let mut table = self.table.lock().unwrap();
        let width = width.clamp(1, table.gpus.len().max(1));
        table.roots.entry(root)
            .or_insert(Root { width, lane: None, released: false, started: false, reported: None, holders: 0 })
            .holders += 1;
        Hold { table: self.clone(), root }
    }
    pub(crate) fn registered(&self, root: u64) -> bool {
        self.table.lock().unwrap().roots.contains_key(&root)
    }
    /// Its lane while bound; `Some(None)` once released, `None` while not yet bound.
    pub(crate) fn bound(&self, root: u64) -> Option<Option<Range<usize>>> {
        let table = self.table.lock().unwrap();
        let root = table.roots.get(&root)?;
        match (&root.lane, root.released) {
            (Some(lane), _) => Some(Some(lane.clone())),
            (None, true) => Some(None),
            (None, false) => None,
        }
    }
    /// A registered root's width (0: none).
    pub(crate) fn width(&self, root: u64) -> usize {
        self.table.lock().unwrap().roots.get(&root).map_or(0, |r| r.width)
    }
    /// Keep the lane `root` would take from later requests in this pass; the request it waits
    /// behind.
    pub(crate) fn claim(&self, root: u64, width: usize, claims: &mut Claims) -> Option<u64> {
        let table = self.table.lock().unwrap();
        let width = width.min(table.gpus.len());
        let start = (0..=table.gpus.len() - width)
            .max_by_key(|start| {
                let free = (*start..start + width).filter(|d| table.free(*d, root, claims)).count();
                (free, std::cmp::Reverse(*start))
            })
            .unwrap_or(0);
        let behind = table.other(start..start + width, root, claims);
        for claim in claims.iter_mut().skip(start).take(width) {
            claim.get_or_insert(root);
        }
        behind
    }
    /// `root`'s lane for its ready step, binding the first lane of its width whose GPUs are
    /// free and claimed by no older request. Else, for its first step, it waits for a lane no
    /// older root holds: its younger holders yield at their next step boundary (those between
    /// steps now: returned with the lanes that ended). Otherwise it waits behind the oldest
    /// blocker, keeping only free GPUs from later requests.
    pub(crate) fn lane(&self, root: u64, claims: &mut Claims) -> (Lane, Vec<(u64, Range<usize>)>) {
        let mut table = self.table.lock().unwrap();
        let Some(entry) = table.roots.get(&root) else {
            return (Lane::Waiting { behind: None, blocker: Blocker::Machine, new: false }, vec![]);
        };
        if let Some(lane) = &entry.lane {
            return (Lane::Held { lane: lane.clone(), new: false }, vec![]);
        }
        let (width, first) = (entry.width.min(table.gpus.len()), !entry.started);
        let starts = 0..=table.gpus.len() - width;
        if let Some(start) = starts.clone().find(|start| (*start..start + width).all(|d| table.free(d, root, claims))) {
            table.bind(root, start..start + width);
            return (Lane::Held { lane: start..start + width, new: true }, vec![]);
        }
        let (obtainable, _, start) = starts
            .map(|start| {
                let lane = start..start + width;
                let free = lane.clone().filter(|d| table.free(*d, root, claims)).count();
                (lane.clone().all(|d| table.obtainable(d, root, first, claims)), free, std::cmp::Reverse(start))
            })
            .max()
            .expect("a lane of its width");
        let lane = start.0..start.0 + width;
        let mut yielded = vec![];
        if obtainable {
            let mut holders: Vec<u64> = lane.clone().filter_map(|d| table.gpus[d].root).collect();
            holders.dedup();
            holders.retain(|holder| table.between_steps(*holder));
            for holder in holders {
                let entry = table.roots.get_mut(&holder).expect("a holder is registered");
                let ended = entry.lane.take().expect("a holder has a lane");
                ended.clone().for_each(|d| table.gpus[d].root = None);
                yielded.push((holder, ended));
            }
            if lane.clone().all(|d| table.free(d, root, claims)) {
                table.bind(root, lane.clone());
                drop(table);
                self.freed();
                return (Lane::Held { lane, new: true }, yielded);
            }
        }
        let behind = table.other(lane.clone(), root, claims);
        let blocker = table.blocker(lane.clone(), root, claims);
        for d in lane {
            if obtainable || table.gpus[d].root.is_none_or(|holder| holder < root) {
                claims[d].get_or_insert(root);
            }
        }
        let entry = table.roots.get_mut(&root).expect("a registered root");
        let new = entry.reported.replace(behind) != Some(behind);
        drop(table);
        if !yielded.is_empty() {
            self.freed();
        }
        (Lane::Waiting { behind, blocker, new }, yielded)
    }
    /// End `root`'s lane now; a call in flight keeps its GPUs until it returns. The lane ended.
    pub(crate) fn release(&self, root: u64) -> Option<Range<usize>> {
        self.release_when(root, false)
    }
    /// A child waiting on sources must not clear the lane of its still-running GPU parent.
    pub(crate) fn release_between_steps(&self, root: u64) -> Option<Range<usize>> {
        self.release_when(root, true)
    }
    fn release_when(&self, root: u64, idle_only: bool) -> Option<Range<usize>> {
        let mut table = self.table.lock().unwrap();
        if idle_only && !table.between_steps(root) { return None; }
        let entry = table.roots.get_mut(&root)?;
        entry.released = true;
        let lane = entry.lane.take()?;
        lane.clone().for_each(|d| table.gpus[d].root = None);
        drop(table);
        self.freed();
        Some(lane)
    }
    /// A call on `devices`: inside `root`'s lane when it is registered, else on GPUs no root
    /// holds or older request claims (a standalone call: its own root; `None`: machine
    /// warming). No GPU may be calling outside `family`. A yielding root's call runs only when
    /// it continues a step in flight.
    pub(crate) fn call(self: &Arc<Self>, root: Option<u64>, family: Option<&str>, devices: Range<usize>, claims: &Claims) -> Option<Permit> {
        let mut table = self.table.lock().unwrap();
        // External pressure holds new roots and warming, but never strands a running parent
        // awaiting its child. Cancellation of that family is a separate durable authority.
        if table.gpus.get(devices.clone())?.iter().any(|gpu| gpu.external_pressure)
            && !table.continuing(family) { return None }
        let registered = root.is_some_and(|root| table.roots.contains_key(&root));
        let usable = !devices.is_empty() && devices.end <= table.gpus.len() && devices.clone().all(|d| {
            let gpu = &table.gpus[d];
            let placed = match registered {
                true => gpu.root == root,
                false => gpu.root.is_none() && claims.get(d).copied().flatten().is_none_or(|c| Some(c) == root),
            };
            let shared = family.is_some() && gpu.caller.as_deref() == family;
            placed && (gpu.calling == 0 || shared)
        }) && !(registered && root.is_some_and(|root| table.yielding(root, claims)) && !table.continuing(family));
        if !usable {
            return None;
        }
        for d in devices.clone() {
            let gpu = &mut table.gpus[d];
            (gpu.calling, gpu.caller, gpu.running) = (gpu.calling + 1, family.map(String::from), root);
        }
        Some(Permit { table: self.clone(), devices, hint: false })
    }
    /// Whether request `seq` still holds a lane or a call slot anywhere.
    pub(crate) fn holds(&self, seq: u64) -> bool {
        self.table.lock().unwrap().gpus.iter().any(|gpu| gpu.root == Some(seq) || gpu.running == Some(seq))
    }
    /// Machine upkeep on `devices` (ending idle executors): no call there.
    pub(crate) fn upkeep(self: &Arc<Self>, devices: Range<usize>) -> Option<Permit> {
        let mut table = self.table.lock().unwrap();
        if devices.clone().any(|d| table.gpus.get(d).is_none_or(|gpu| gpu.calling > 0)) {
            return None;
        }
        devices.clone().for_each(|d| table.gpus[d].calling += 1);
        Some(Permit { table: self.clone(), devices, hint: false })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut table = self.table.table.lock().unwrap();
        for gpu in &mut table.gpus[self.devices.clone()] {
            gpu.calling -= 1;
            gpu.hints -= usize::from(self.hint);
            if gpu.calling == 0 {
                (gpu.caller, gpu.running) = (None, None);
            }
        }
        drop(table);
        self.table.freed();
    }
}
impl Permit {
    /// This call is a prefetch's prewarm, which ends with its owner: a root waiting behind it
    /// keeps waiting (`Blocker::Request { hint }`).
    pub(crate) fn hint(&mut self) {
        if self.hint { return }
        self.hint = true;
        let mut table = self.table.table.lock().unwrap();
        self.devices.clone().for_each(|d| table.gpus[d].hints += 1);
    }
}
impl Hold {
    pub(crate) fn root(&self) -> u64 {
        self.root
    }
}
impl Drop for Hold {
    fn drop(&mut self) {
        let mut table = self.table.table.lock().unwrap();
        let ended = table.roots.get_mut(&self.root).is_some_and(|root| {
            root.holders -= 1;
            root.holders == 0
        });
        if ended {
            table.roots.remove(&self.root);
            table.gpus.iter_mut().filter(|gpu| gpu.root == Some(self.root)).for_each(|gpu| gpu.root = None);
        }
        drop(table);
        self.table.freed();
    }
}
