//! Persistent observer custody and event-scoped final Arena resources.
//! A request never revokes genuine Region/Pin/Fill/Plan/transient owners.
use crate::{Arena, Contents, Error, Result, Room, State, cuda, retirement};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

const UNPROVEN: &str = "arena retirement has unproven consumer completion";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArenaRetirementState {
    Pending,
    Complete,
    Unproven,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ArenaRetirementFacts {
    pub arena_id: u64,
    pub device: i32,
    pub state: ArenaRetirementState,
    pub observers_detached: bool,
    pub remaining_physical_bytes: usize,
    pub failure: Option<String>,
}

struct Status {
    state: ArenaRetirementState,
    observers_detached: bool,
    failure: Option<String>,
}

pub(crate) struct Control {
    pid: u32,
    id: u64,
    device: i32,
    physical: Arc<AtomicUsize>,
    status: Mutex<Status>,
}

/// Host-only completion ticket. Native queue/genuine owners retain every GPU resource
/// even if the caller drops this ticket. Polling never performs a CUDA call or wait.
pub struct ArenaRetirement {
    control: Arc<Control>,
}

impl ArenaRetirement {
    pub fn facts(&self) -> Result<ArenaRetirementFacts> {
        crate::check_process()?;
        if self.control.pid != std::process::id() {
            return Err(Error::Invalid("arena retirement inherited after fork"));
        }
        let state = self
            .control
            .status
            .lock()
            .map_err(|_| Error::Poisoned("arena retirement status mutex".into()))?;
        Ok(ArenaRetirementFacts {
            arena_id: self.control.id,
            device: self.control.device,
            state: state.state,
            observers_detached: state.observers_detached,
            remaining_physical_bytes: self.control.physical.load(Ordering::Acquire),
            failure: state.failure.clone(),
        })
    }

    pub fn try_complete(&self) -> Result<bool> {
        let facts = self.facts()?;
        match facts.state {
            ArenaRetirementState::Pending => Ok(false),
            ArenaRetirementState::Complete => Ok(true),
            ArenaRetirementState::Unproven => Err(Error::Invalid(UNPROVEN)),
            ArenaRetirementState::Failed => Err(Error::Poisoned(
                facts
                    .failure
                    .unwrap_or_else(|| "arena retirement failed".into()),
            )),
        }
    }
}

#[derive(Default)]
struct Observers {
    closing: bool,
    ticket: Option<Arc<Control>>,
    leases: Vec<Weak<Observation>>,
}

/// Registration metadata owns no Driver or GPU state. Finalizing remaining bytes
/// stay visible until actual releases, including unproven/fault quarantine.
pub(crate) struct Metadata {
    pub(crate) id: u64,
    pub(crate) device: i32,
    pub(crate) physical: Arc<AtomicUsize>,
    pub(crate) finalizing: AtomicBool,
    counters: Arc<cuda::Counters>,
    observers: Mutex<Observers>,
}

impl Metadata {
    pub(crate) fn new(id: u64, device: i32, counters: Arc<cuda::Counters>) -> Arc<Self> {
        Arc::new(Self {
            id,
            device,
            physical: Arc::new(AtomicUsize::new(0)),
            finalizing: AtomicBool::new(false),
            counters,
            observers: Mutex::new(Observers::default()),
        })
    }

    fn control(&self, held: &mut Observers) -> Arc<Control> {
        held.ticket
            .get_or_insert_with(|| {
                Arc::new(Control {
                    pid: std::process::id(),
                    id: self.id,
                    device: self.device,
                    physical: self.physical.clone(),
                    status: Mutex::new(Status {
                        state: ArenaRetirementState::Pending,
                        observers_detached: false,
                        failure: None,
                    }),
                })
            })
            .clone()
    }

    pub(crate) fn observe(&self, arena: Option<&Arc<Arena>>) -> Result<Arc<Observation>> {
        let mut held = self
            .observers
            .lock()
            .map_err(|_| Error::Poisoned("arena observer admission mutex".into()))?;
        let owner = if held.closing { None } else { arena.cloned() };
        let observation = Arc::new(Observation {
            id: self.id,
            counters: self.counters.clone(),
            owner: Mutex::new(owner),
        });
        held.leases.retain(|lease| lease.strong_count() != 0);
        held.leases.push(Arc::downgrade(&observation));
        Ok(observation)
    }
}

/// Persistent GPU custody is explicit and detachable. Historical counter metadata
/// remains host-only after detach, so a native-call epoch cannot decrease or alias.
pub(crate) struct Observation {
    pub(crate) id: u64,
    counters: Arc<cuda::Counters>,
    owner: Mutex<Option<Arc<Arena>>>,
}

impl Observation {
    pub(crate) fn activity_calls(&self) -> u64 {
        self.counters.activity_calls.load(Ordering::Relaxed)
    }
    pub(crate) fn memory_calls(&self) -> u64 {
        self.counters.memory_calls.load(Ordering::Relaxed)
    }
}

pub(crate) fn request(
    arena: Arc<Arena>,
) -> std::result::Result<ArenaRetirement, (Arc<Arena>, Error)> {
    if let Err(error) = crate::check_process().and_then(|()| retirement::prepare(arena.device())) {
        return Err((arena, error));
    }
    let metadata = arena.metadata.clone();
    let mut held = match metadata.observers.lock() {
        Ok(held) => held,
        Err(_) => {
            return Err((
                arena,
                Error::Poisoned("arena observer admission mutex".into()),
            ));
        }
    };
    if let Some(control) = &held.ticket {
        let ticket = ArenaRetirement {
            control: control.clone(),
        };
        drop(held);
        drop(arena); // Another accepted job or genuine owner keeps custody; Drop itself only queues.
        return Ok(ticket);
    }
    let control = Arc::new(Control {
        pid: std::process::id(),
        id: arena.id(),
        device: arena.device(),
        physical: metadata.physical.clone(),
        status: Mutex::new(Status {
            state: ArenaRetirementState::Pending,
            observers_detached: false,
            failure: None,
        }),
    });
    let job = retirement::Job::Arena(Payload::Detach {
        arena: Some(arena.clone()),
        control: control.clone(),
    });
    // The worker cannot inspect this arena's observer set until this admission lock
    // releases. Failed enqueue leaves admission unchanged and returns the input owner.
    match retirement::enqueue(arena.device(), job) {
        Ok(()) => {
            held.closing = true;
            held.ticket = Some(control.clone());
            drop(held);
            drop(arena);
            Ok(ArenaRetirement { control })
        }
        Err((error, job)) => {
            drop(held);
            drop(job); // The original input Arc remains owned below.
            Err((arena, error))
        }
    }
}

/// Destructor adapter: current-process handoff failures keep the exact Arena in
/// the existing native quarantine and return its failed, host-only ticket. A fork
/// rejection precedes every inherited mutex and returns ownership to the caller.
pub(crate) fn defer(
    arena: Arc<Arena>,
) -> std::result::Result<ArenaRetirement, (Arc<Arena>, Error)> {
    if arena.pid != std::process::id() {
        return Err((
            arena,
            Error::Invalid("arena retirement inherited after fork"),
        ));
    }
    match request(arena) {
        Ok(ticket) => Ok(ticket),
        Err((arena, error)) => {
            let metadata = arena.metadata.clone();
            let control = {
                // Recover poisoned metadata only to preserve owned resources, not to
                // grant further work or reinterpret the failed operation as successful.
                let mut held = metadata.observers.lock().unwrap_or_else(|e| e.into_inner());
                held.closing = true;
                metadata.control(&mut held)
            };
            let ticket = ArenaRetirement {
                control: control.clone(),
            };
            let device = arena.device();
            retirement::quarantine(
                device,
                retirement::Job::Arena(Payload::Detach {
                    arena: Some(arena),
                    control,
                }),
                &error,
            );
            Ok(ticket)
        }
    }
}

pub(crate) enum Payload {
    Detach {
        arena: Option<Arc<Arena>>,
        control: Arc<Control>,
    },
    Resources(Resources),
}

pub(crate) struct Resources {
    pid: u32,
    metadata: Arc<Metadata>,
    driver: Arc<cuda::Driver>,
    room: Arc<Room>,
    state: Option<State>,
    control: Arc<Control>,
}

impl Payload {
    pub(crate) fn poll(&mut self) -> Result<bool> {
        match self {
            Self::Detach { arena, control } => {
                let Some(owner) = arena.as_ref() else {
                    return Ok(true);
                };
                let _retirement = match crate::enter_retirement(owner.device()) {
                    Ok(guard) => guard,
                    Err(Error::Busy | Error::CaptureActive) => return Ok(false),
                    Err(error) => return Err(error),
                };
                let observers = {
                    let held =
                        owner.metadata.observers.lock().map_err(|_| {
                            Error::Poisoned("arena observer admission mutex".into())
                        })?;
                    held.leases
                        .iter()
                        .filter_map(Weak::upgrade)
                        .collect::<Vec<_>>()
                };
                let mut busy = false;
                for observer in observers {
                    let released = match observer.owner.try_lock() {
                        Ok(mut held) => held.take(),
                        Err(std::sync::TryLockError::WouldBlock) => {
                            busy = true;
                            None
                        }
                        Err(_) => return Err(Error::Poisoned("arena observer lease mutex".into())),
                    };
                    drop(released); // No mutex is held; request retains its genuine Arc.
                }
                if busy {
                    return Ok(false);
                }
                drop(arena.take()); // Remaining real owners determine when final Drop occurs.
                control
                    .status
                    .lock()
                    .map_err(|_| Error::Poisoned("arena retirement status mutex".into()))?
                    .observers_detached = true;
                Ok(true)
            }
            Self::Resources(resources) => resources.poll(),
        }
    }

    pub(crate) fn failed(&mut self, error: &Error) {
        let control = match self {
            Self::Detach { control, .. } => control,
            Self::Resources(resources) => &resources.control,
        };
        let mut state = control.status.lock().unwrap_or_else(|e| e.into_inner());
        if state.failure.is_none() {
            state.state = if matches!(error,Error::Invalid(reason) if *reason==UNPROVEN) {
                ArenaRetirementState::Unproven
            } else {
                ArenaRetirementState::Failed
            };
            state.failure = Some(error.to_string());
        }
    }
}

/// Last Arc handoff: owned native State/Driver move to the worker; no CUDA call,
/// capture-mutex wait or physical-release claim occurs on the dropping stack.
pub(crate) fn final_owner(arena: &mut Arena) {
    let state = std::mem::replace(
        arena.state.get_mut().unwrap_or_else(|e| e.into_inner()),
        State::empty(),
    );
    let control = {
        let mut observers = arena
            .metadata
            .observers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        observers.closing = true;
        let control = arena.metadata.control(&mut observers);
        control
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .observers_detached = true;
        control
    };
    let resources = Resources {
        pid: arena.pid,
        metadata: arena.metadata.clone(),
        driver: arena.driver.clone(),
        room: arena.room.clone(),
        state: Some(state),
        control,
    };
    // Last-Arc ownership means no reader can hold this arena. Serialize the accepted
    // handoff and epoch under the registration mutex, without a global GPU exclusion.
    if let Some(registry) = crate::ARENAS.get() {
        let registry = registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.iter().any(|entry| entry.id == arena.id) {
            let _ = crate::plan::inventory_changed(arena.device);
        }
        arena.metadata.finalizing.store(true, Ordering::Release);
        drop(registry);
    } else {
        arena.metadata.finalizing.store(true, Ordering::Release);
    }
    retirement::defer(
        arena.device,
        retirement::Job::Arena(Payload::Resources(resources)),
    );
}

impl Resources {
    fn poll(&mut self) -> Result<bool> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid("arena resources inherited after fork"));
        }
        let Some(state) = self.state.as_mut() else {
            return Ok(true);
        };
        if let Some(reason) = &state.poison {
            return Err(Error::Poisoned(reason.clone()));
        }
        // Genuine pins/fills must keep their Arena alive. Unknown last consumers are
        // quarantined instead of silently joining all device/context work.
        if state.entries.values().any(|entry| {
            entry.uses.pins() != 0
                || !matches!(
                    entry.uses.state(),
                    Contents::Cold | Contents::Ready | Contents::Retired
                )
                || entry.uses.unjoined.load(Ordering::Acquire)
        }) {
            return Err(Error::Invalid(UNPROVEN));
        }
        let _retirement = match crate::enter_retirement(self.metadata.device) {
            Ok(guard) => guard,
            Err(Error::Busy | Error::CaptureActive) => return Ok(false),
            Err(error) => return Err(error),
        };
        let _context = self.driver.enter()?;
        // Event snapshots own only integer handles; no event mutex crosses a driver call.
        for entry in state.entries.values() {
            let fences = entry.uses.fences().clone();
            for fence in fences {
                if !self.driver.query(fence.event)? {
                    return Ok(false);
                }
            }
        }
        let ids = state.entries.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let entry = state
                .entries
                .get_mut(&id)
                .expect("exclusive final resource entry");
            loop {
                let fence = entry.uses.fences().last().copied();
                let Some(fence) = fence else {
                    break;
                };
                self.driver.destroy_event(fence.event)?;
                entry.uses.fences().pop();
            }
            if let Some(handle) = entry.handle {
                self.driver.unmap(entry.ptr, entry.bytes)?;
                self.driver.release(handle)?;
                entry.handle = None; // Record the actual effect before a later fallible VA release.
                self.metadata
                    .physical
                    .fetch_sub(entry.bytes, Ordering::AcqRel);
                self.room.pages.fetch_sub(entry.bytes, Ordering::AcqRel);
                self.room
                    .released
                    .fetch_add(entry.bytes as u64, Ordering::AcqRel);
            }
            self.driver.free_address(entry.ptr, entry.bytes)?;
            state.entries.remove(&id);
        }
        if self.metadata.physical.load(Ordering::Acquire) != 0 {
            return Err(Error::Poisoned(
                "arena finalization retains unattributed physical handles".into(),
            ));
        }
        // Finalizing metadata is not live intent. Its logical retirement epoch was
        // already published at handoff; removing a zero-byte record cannot reuse it.
        if let Some(registry) = crate::ARENAS.get() {
            let mut registry = registry
                .lock()
                .map_err(|_| Error::Poisoned("arena registry mutex".into()))?;
            registry.retain(|entry| entry.id != self.metadata.id);
        }
        drop(_context);
        self.state = None;
        self.control
            .status
            .lock()
            .map_err(|_| Error::Poisoned("arena retirement status mutex".into()))?
            .state = ArenaRetirementState::Complete;
        Ok(true)
    }
}
