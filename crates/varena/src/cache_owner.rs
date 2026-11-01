//! Nonwaiting requests to explicit cache owners. Allocators see host control only;
//! callback/data ownership stays on a separate worker, never the cooperation thread.
use crate::{Error, GlobalCaptureGuard, Result};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Allocation,
    Cooperative,
    Foreign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestKey {
    pub cause: Cause,
    /// Process-unique intent identity from next_generation(), not a reusable ledger round.
    /// The cooperation adapter retains the association with its exact ledger round itself.
    pub generation: u64,
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);
pub fn next_generation() -> Result<u64> {
    crate::check_process()?;
    NEXT_REQUEST
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map(|previous| previous + 1)
        .map_err(|_| Error::Invalid("cache request identity overflow"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackResult {
    Returned,
    Declined,
}

/// Narrow foreign-runtime adapter. Implementations acquire their runtime/GIL first,
/// then the appropriate scope before touching opaque GPU owners. Retirement must empty owned
/// callback data before returning success, so destroying the adapter itself is host-only.
#[doc(hidden)]
pub trait CacheCallback: Send + 'static {
    fn shrink(&mut self, scope: &CallbackScope, bytes: usize) -> Result<CallbackResult>;
    fn retire(&mut self, scope: &CallbackScope) -> Result<()>;
}

pub struct CallbackScope {
    control: Arc<Control>,
    owner: Arc<OwnerMeta>,
    job: Option<Arc<Job>>,
}

/// Counter leases only: no capture/allocator/ledger mutex is held across a callback.
pub struct CallbackGuard {
    _callback: GlobalCaptureGuard,
    _retirement: Option<GlobalCaptureGuard>,
    _producer: Option<crate::ProducerGuard>,
}

impl CallbackScope {
    pub fn device(&self) -> i32 {
        self.control.device
    }

    pub fn current(&self) -> Result<bool> {
        let owner = self.owner.lock()?;
        Ok(if let Some(job) = &self.job {
            !owner.closing && owner.facts.requestable && !job.lock()?.cancelled
        } else {
            owner.closing
        })
    }

    /// Full retirement exclusion for callbacks that can destroy their last GPU owner.
    pub fn enter(&self) -> Result<CallbackGuard> {
        if !self.current()? {
            return Err(Error::Busy);
        }
        let retirement = crate::enter_retirement(self.control.device)?;
        let callback = crate::enter_external_callback(self.control.device)?;
        Ok(CallbackGuard {
            _callback: callback,
            _retirement: Some(retirement),
            _producer: None,
        })
    }

    /// Logical owner retirement with actual per-owner locks/pins, allowing other graph
    /// sequences to replay. The worker retains observed Arena leases separately; physical
    /// unmapping remains in the ready-only pass after this scope and defers during replay.
    pub fn producing(&self) -> Result<CallbackGuard> {
        if !self.current()? {
            return Err(Error::Busy);
        }
        let producer = crate::enter_producer(self.control.device)?;
        let callback = crate::enter_external_callback(self.control.device)?;
        Ok(CallbackGuard {
            _callback: callback,
            _retirement: None,
            _producer: Some(producer),
        })
    }
}

struct NativeCallback<F>(Option<F>);
impl<F: FnMut(usize) -> Result<CallbackResult> + Send + 'static> CacheCallback
    for NativeCallback<F>
{
    fn shrink(&mut self, scope: &CallbackScope, bytes: usize) -> Result<CallbackResult> {
        let _guard = scope.enter()?;
        self.0
            .as_mut()
            .ok_or(Error::Invalid("cache callback is retired"))?(bytes)
    }
    fn retire(&mut self, scope: &CallbackScope) -> Result<()> {
        let _guard = scope.enter()?;
        drop(self.0.take());
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct OwnerFacts {
    pub id: u64,
    pub registered: bool,
    pub closing: bool,
    pub running: bool,
    pub retired: bool,
    pub callbacks: u64,
    pub returned: u64,
    pub declined: u64,
    pub deferred: u64,
    pub failed: Option<String>,
    pub retirement_failed: Option<String>,
    pub requestable: bool,
}

struct OwnerState {
    facts: OwnerFacts,
    closing: bool,
    last_use: u64,
}
struct OwnerMeta {
    pid: u32,
    state: Mutex<OwnerState>,
}
impl OwnerMeta {
    fn lock(&self) -> Result<MutexGuard<'_, OwnerState>> {
        crate::check_process()?;
        if self.pid != std::process::id() {
            return Err(Error::Invalid("cache owner used after fork"));
        }
        self.state
            .lock()
            .map_err(|_| Error::Poisoned("cache owner metadata mutex".into()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct RequestFacts {
    pub queued: usize,
    pub running: bool,
    pub returned: u64,
    pub declined: u64,
    pub failed: u64,
    pub finished: bool,
    pub cancelled: bool,
    /// Actual global physical releases observed during this request, never owner attribution.
    pub observed_release_bytes: u64,
}

struct JobState {
    wanted: usize,
    owners: VecDeque<u64>,
    facts: RequestFacts,
    // Cooperative/foreign callers advance only after their ordinary capped native service.
    waiting_service: bool,
    cancelled: bool,
    collect_pending: bool,
}
struct Job {
    key: RequestKey,
    start_release: u64,
    state: Mutex<JobState>,
}
impl Job {
    fn lock(&self) -> Result<MutexGuard<'_, JobState>> {
        self.state
            .lock()
            .map_err(|_| Error::Poisoned("cache request metadata mutex".into()))
    }
}

struct State {
    next_owner: u64,
    owners: BTreeMap<u64, Arc<OwnerMeta>>,
    jobs: BTreeMap<RequestKey, Arc<Job>>,
    last: BTreeMap<Cause, Arc<Job>>,
    worker: Option<ThreadId>,
}
struct Control {
    pid: u32,
    device: i32,
    state: Mutex<State>,
    changed: Condvar,
    clock: AtomicU64,
    requestable: AtomicUsize,
    room: Arc<crate::Room>,
}
impl Control {
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        crate::check_process()?;
        if self.pid != std::process::id() {
            return Err(Error::Invalid("cache dispatcher used after fork"));
        }
        self.state
            .lock()
            .map_err(|_| Error::Poisoned("cache dispatcher metadata mutex".into()))
    }
}

struct Registration {
    id: u64,
    owner: Arc<OwnerMeta>,
    callback: Box<dyn CacheCallback>,
}
struct Endpoint {
    control: Arc<Control>,
    registration: mpsc::Sender<Registration>,
}
static REGISTRY: OnceLock<Mutex<BTreeMap<i32, Endpoint>>> = OnceLock::new();

fn registry() -> Result<MutexGuard<'static, BTreeMap<i32, Endpoint>>> {
    crate::check_process()?;
    REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("cache dispatcher registry".into()))
}
fn control(device: i32) -> Result<Option<Arc<Control>>> {
    Ok(registry()?
        .get(&device)
        .map(|endpoint| Arc::clone(&endpoint.control)))
}

/// Host-only handle. Its finalizer merely closes request admission and wakes the owner worker.
pub struct CacheOwner {
    control: Arc<Control>,
    owner: Arc<OwnerMeta>,
    id: u64,
}
impl CacheOwner {
    pub fn new<F>(device: i32, callback: F) -> Result<Self>
    where
        F: FnMut(usize) -> Result<CallbackResult> + Send + 'static,
    {
        Self::with_callback(device, Box::new(NativeCallback(Some(callback))))
    }

    #[doc(hidden)]
    pub fn with_callback(device: i32, callback: Box<dyn CacheCallback>) -> Result<Self> {
        if device < 0 {
            return Err(Error::Invalid("cache owner device must be nonnegative"));
        }
        // Capture metadata is acquired before the dispatcher registry, never under it.
        let room = crate::room(device)?;
        let (control, registration) = {
            let mut registry = registry()?;
            if !registry.contains_key(&device) {
                let control = Arc::new(Control {
                    pid: std::process::id(),
                    device,
                    state: Mutex::new(State {
                        next_owner: 0,
                        owners: BTreeMap::new(),
                        jobs: BTreeMap::new(),
                        last: BTreeMap::new(),
                        worker: None,
                    }),
                    changed: Condvar::new(),
                    clock: AtomicU64::new(0),
                    requestable: AtomicUsize::new(0),
                    room,
                });
                let (send, receive) = mpsc::channel();
                let thread_control = Arc::clone(&control);
                std::thread::Builder::new()
                    .name(format!("varena-cache-{device}"))
                    .spawn(move || worker(thread_control, receive))
                    .map_err(|error| {
                        Error::Unsupported(format!("cache callback thread: {error}"))
                    })?;
                registry.insert(
                    device,
                    Endpoint {
                        control,
                        registration: send,
                    },
                );
            }
            let endpoint = &registry[&device];
            (Arc::clone(&endpoint.control), endpoint.registration.clone())
        };
        let (id, owner) = {
            let mut state = control.lock()?;
            let id = state
                .next_owner
                .checked_add(1)
                .ok_or(Error::Invalid("cache owner identity overflow"))?;
            state.next_owner = id;
            let owner = Arc::new(OwnerMeta {
                pid: std::process::id(),
                state: Mutex::new(OwnerState {
                    facts: OwnerFacts {
                        id,
                        ..OwnerFacts::default()
                    },
                    closing: false,
                    last_use: control.clock.fetch_add(1, Ordering::AcqRel),
                }),
            });
            state.owners.insert(id, Arc::clone(&owner));
            (id, owner)
        };
        // The registration channel is never reachable through allocator Control handles.
        if registration
            .send(Registration {
                id,
                owner: Arc::clone(&owner),
                callback,
            })
            .is_err()
        {
            control.lock()?.owners.remove(&id);
            return Err(Error::Poisoned("cache callback worker ended".into()));
        }
        {
            let mut held = owner.lock()?;
            held.facts.registered = true;
            held.facts.requestable = true;
            control.requestable.fetch_add(1, Ordering::AcqRel);
        }
        control.changed.notify_all();
        Ok(Self { control, owner, id })
    }

    pub fn touch(&self) -> Result<()> {
        let mut owner = self.owner.lock()?;
        if owner.closing {
            return Err(Error::Invalid("cache owner is closed"));
        }
        owner.last_use = self.control.clock.fetch_add(1, Ordering::AcqRel);
        if owner.facts.registered
            && !owner.facts.requestable
            && owner.facts.failed.is_none()
            && owner.facts.retirement_failed.is_none()
        {
            owner.facts.requestable = true;
            self.control.requestable.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
    /// The owner has no disposable contents until its next actual use/rebuild.
    pub fn suspend(&self) -> Result<()> {
        let mut owner = self.owner.lock()?;
        if owner.facts.requestable {
            owner.facts.requestable = false;
            self.control.requestable.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(())
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn facts(&self) -> Result<OwnerFacts> {
        Ok(self.owner.lock()?.facts.clone())
    }
    pub fn check(&self) -> Result<()> {
        let owner = self.owner.lock()?;
        if let Some(reason) = owner
            .facts
            .failed
            .as_ref()
            .or(owner.facts.retirement_failed.as_ref())
        {
            return Err(Error::Callback(reason.clone()));
        }
        if owner.closing {
            return Err(Error::Invalid("cache owner is closed"));
        }
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        let mut owner = self.owner.lock()?;
        if !owner.closing {
            owner.closing = true;
            owner.facts.closing = true;
            if owner.facts.requestable {
                owner.facts.requestable = false;
                self.control.requestable.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.control.changed.notify_all();
        Ok(())
    }
    /// Close admission without waiting. Runtime may use this while holding its own
    /// owner lock, then release that lock before the explicit drain in close().
    pub fn defer_close(&self) -> Result<()> {
        self.stop()
    }
    /// Explicit callers must release their GIL/owner locks while draining. Inside a callback
    /// or capture this queues retirement and refuses; the worker retains the intact callback.
    pub fn close(&self) -> Result<()> {
        self.stop()?;
        if self.control.lock()?.worker == Some(std::thread::current().id()) {
            return Err(Error::Busy);
        }
        loop {
            if crate::is_recovery_blocked(self.control.device)? {
                return Err(Error::CaptureActive);
            }
            let facts = self.facts()?;
            if facts.retired {
                return Ok(());
            }
            if let Some(reason) = facts.retirement_failed {
                return Err(Error::Poisoned(reason));
            }
            let state = self.control.lock()?;
            drop(
                self.control
                    .changed
                    .wait_timeout(state, Duration::from_millis(20))
                    .map_err(|_| Error::Poisoned("cache close wait mutex".into()))?,
            );
        }
    }
}
impl Drop for CacheOwner {
    fn drop(&mut self) {
        if self.control.pid == std::process::id() {
            let _ = self.stop();
        }
    }
}

pub fn requestable(device: i32) -> Result<usize> {
    Ok(control(device)?.map_or(0, |control| control.requestable.load(Ordering::Acquire)))
}

/// A caller may observe this handle; it owns host data only and grants no capacity.
pub struct ShrinkRequest {
    control: Arc<Control>,
    job: Arc<Job>,
}
impl ShrinkRequest {
    fn check(&self) -> Result<()> {
        crate::check_process()?;
        if self.control.pid != std::process::id() {
            return Err(Error::Invalid("cache request used after fork"));
        }
        Ok(())
    }
    pub fn facts(&self) -> Result<RequestFacts> {
        self.check()?;
        let mut job = self.job.lock()?;
        job.facts.observed_release_bytes = self
            .control
            .room
            .released
            .load(Ordering::Acquire)
            .checked_sub(self.job.start_release)
            .unwrap_or(0);
        Ok(job.facts.clone())
    }
    pub fn cancel(&self) -> Result<()> {
        self.check()?;
        let mut job = self.job.lock()?;
        job.cancelled = true;
        job.facts.cancelled = true;
        self.control.changed.notify_all();
        Ok(())
    }
}

/// Queue only. Repeated observations coalesce by cause/generation, never add promised bytes.
/// Cooperative/foreign callers repeat this only after their ordinary capped service pass;
/// that permits the next owner action without crediting the previous callback's return.
pub fn request_shrink(device: i32, key: RequestKey, bytes: usize) -> Result<Option<ShrinkRequest>> {
    let Some(control) = control(device)? else {
        return Ok(None);
    };
    let start_release = crate::released_bytes(device)?;
    let mut state = control.lock()?;
    if state.worker == Some(std::thread::current().id()) {
        return Ok(None);
    }
    // Concurrent/rapid local failures share one outstanding owner pass. A new actual
    // refusal after earlier physical releases asks for remaining progress, not a sum
    // of duplicate deficits. No allocator waits for this pass or its callback thread.
    if key.cause == Cause::Allocation {
        for pending in state
            .jobs
            .values()
            .filter(|job| job.key.cause == Cause::Allocation)
        {
            let mut held = pending.lock()?;
            if !held.cancelled && !held.facts.finished && !held.owners.is_empty() {
                let observed = start_release
                    .checked_sub(pending.start_release)
                    .unwrap_or(0);
                held.wanted = held.wanted.max(bytes.saturating_add(observed as usize));
                let job = Arc::clone(pending);
                drop(held);
                drop(state);
                control.changed.notify_all();
                return Ok(Some(ShrinkRequest { control, job }));
            }
        }
    }
    if control.requestable.load(Ordering::Acquire) == 0 {
        return Ok(None);
    }
    if !state.jobs.contains_key(&key)
        && state
            .last
            .get(&key.cause)
            .is_some_and(|job| job.key.generation > key.generation)
    {
        return Err(Error::Invalid("cache request identity is obsolete"));
    }
    let job = if let Some(job) = state
        .jobs
        .get(&key)
        .or_else(|| state.last.get(&key.cause).filter(|job| job.key == key))
    {
        let job = Arc::clone(job);
        let mut held = job.lock()?;
        if held.facts.finished || held.cancelled {
            if bytes > held.wanted {
                return Err(Error::Invalid("ended cache request cannot grow"));
            }
        } else {
            held.wanted = held.wanted.max(bytes);
            held.waiting_service = false;
        }
        drop(held);
        job
    } else {
        let mut owners = Vec::new();
        for (id, owner) in &state.owners {
            let held = owner.lock()?;
            if held.facts.requestable && !held.closing && held.facts.failed.is_none() {
                owners.push((held.last_use, *id));
            }
        }
        owners.sort_unstable();
        let count = owners.len();
        let job = Arc::new(Job {
            key,
            start_release,
            state: Mutex::new(JobState {
                wanted: bytes,
                owners: owners.into_iter().map(|(_, id)| id).collect(),
                facts: RequestFacts {
                    queued: count,
                    finished: count == 0 || bytes == 0,
                    ..RequestFacts::default()
                },
                waiting_service: false,
                cancelled: false,
                collect_pending: key.cause == Cause::Allocation,
            }),
        });
        state.jobs.insert(key, Arc::clone(&job));
        job
    };
    drop(state);
    control.changed.notify_all();
    Ok(Some(ShrinkRequest { control, job }))
}

fn deferred(error: &Error) -> bool {
    matches!(error, Error::Busy | Error::CaptureActive)
}

fn failed(control: &Control, owner: &mut OwnerState, reason: String, retirement: bool) {
    if owner.facts.requestable {
        owner.facts.requestable = false;
        control.requestable.fetch_sub(1, Ordering::AcqRel);
    }
    if retirement {
        owner.facts.retirement_failed = Some(reason);
    } else {
        owner.facts.failed = Some(reason);
    }
}

fn worker(control: Arc<Control>, registrations: mpsc::Receiver<Registration>) {
    let mut callbacks: BTreeMap<u64, Registration> = BTreeMap::new();
    let mut arenas = Vec::new(); // GPU leases live only here, never in allocator Control.
    if let Ok(mut state) = control.lock() {
        state.worker = Some(std::thread::current().id());
    }
    loop {
        let result = worker_tick(&control, &registrations, &mut callbacks, &mut arenas);
        if let Err(error) = result {
            // Keep callback/data ownership intact on a worker fault. No allocator handle
            // can run these destructors. Explicit close reports failure instead of hanging.
            for registration in callbacks.values() {
                if let Ok(mut owner) = registration.owner.lock() {
                    failed(&control, &mut owner, error.to_string(), true);
                }
            }
            control.changed.notify_all();
        }
        let Ok(state) = control.lock() else {
            std::mem::forget(callbacks);
            std::mem::forget(arenas);
            std::mem::forget(registrations); // queued callback data remains owned too
            return;
        };
        let _ = control
            .changed
            .wait_timeout(state, Duration::from_millis(20));
    }
}

fn worker_tick(
    control: &Arc<Control>,
    registrations: &mpsc::Receiver<Registration>,
    callbacks: &mut BTreeMap<u64, Registration>,
    arenas: &mut Vec<Arc<crate::Arena>>,
) -> Result<()> {
    for registration in registrations.try_iter() {
        callbacks.insert(registration.id, registration);
    }
    let retiring = callbacks
        .iter()
        .filter_map(|(id, registration)| {
            registration
                .owner
                .lock()
                .ok()
                .filter(|owner| {
                    owner.closing && !owner.facts.running && owner.facts.retirement_failed.is_none()
                })
                .map(|_| *id)
        })
        .collect::<Vec<_>>();
    for id in retiring {
        crate::retain_arenas(control.device, arenas)?;
        let registration = callbacks.get_mut(&id).expect("worker-owned callback");
        let scope = CallbackScope {
            control: Arc::clone(control),
            owner: Arc::clone(&registration.owner),
            job: None,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registration.callback.retire(&scope)
        }))
        .unwrap_or_else(|_| Err(Error::Poisoned("cache callback retirement panicked".into())));
        match result {
            Ok(()) => {
                registration.owner.lock()?.facts.retired = true;
                // retire emptied callback data under its own runtime + retirement guard.
                callbacks.remove(&id);
                control.lock()?.owners.remove(&id);
            }
            Err(error) if deferred(&error) => {}
            Err(error) => {
                let mut owner = registration.owner.lock()?;
                failed(control, &mut owner, error.to_string(), true);
            }
        }
        control.changed.notify_all();
    }
    let jobs = control.lock()?.jobs.values().cloned().collect::<Vec<_>>();
    for job in jobs {
        let collect = {
            let held = job.lock()?;
            (!held.cancelled && held.collect_pending).then_some(held.wanted)
        };
        if let Some(wanted) = collect {
            match crate::reclaim_completed(control.device, wanted, arenas) {
                Ok(released) => {
                    job.lock()?.collect_pending = false;
                    #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
                    if released != 0 {
                        crate::cooperative::wake_cache_actions(control.device)?;
                    }
                    #[cfg(not(all(target_os = "linux", target_has_atomic = "64")))]
                    let _ = released;
                }
                Err(error) if deferred(&error) => continue,
                Err(error) => return Err(error),
            }
        }
        let (id, wanted) = {
            let mut held = job.lock()?;
            let observed = control
                .room
                .released
                .load(Ordering::Acquire)
                .checked_sub(job.start_release)
                .unwrap_or(0);
            held.facts.observed_release_bytes = observed;
            if held.cancelled || observed >= held.wanted as u64 || held.owners.is_empty() {
                held.facts.finished = true;
            }
            if held.facts.finished {
                drop(held);
                let mut state = control.lock()?;
                state.jobs.remove(&job.key);
                if state
                    .last
                    .get(&job.key.cause)
                    .is_none_or(|previous| previous.key.generation < job.key.generation)
                {
                    state.last.insert(job.key.cause, Arc::clone(&job));
                }
                continue;
            }
            if held.waiting_service {
                continue;
            }
            let id = held.owners.pop_front().expect("pending owner");
            held.facts.queued = held.owners.len();
            held.facts.running = true;
            (id, held.wanted.saturating_sub(observed as usize))
        };
        crate::retain_arenas(control.device, arenas)?;
        let Some(registration) = callbacks.get_mut(&id) else {
            // Constructor publication can race this tick's registration-channel drain.
            // Keep the accepted request until the already-transferred callback arrives.
            let pending_registration = control.lock()?.owners.contains_key(&id);
            let mut held = job.lock()?;
            held.facts.running = false;
            if pending_registration {
                held.owners.push_front(id);
                held.facts.queued = held.owners.len();
            }
            continue;
        };
        {
            let mut owner = registration.owner.lock()?;
            if owner.closing || !owner.facts.requestable || owner.facts.failed.is_some() {
                job.lock()?.facts.running = false;
                continue;
            }
            owner.facts.running = true;
            owner.facts.callbacks += 1;
        }
        let scope = CallbackScope {
            control: Arc::clone(control),
            owner: Arc::clone(&registration.owner),
            job: Some(Arc::clone(&job)),
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registration.callback.shrink(&scope, wanted)
        }))
        .unwrap_or_else(|_| Err(Error::Poisoned("cache callback panicked".into())));
        let mut owner = registration.owner.lock()?;
        owner.facts.running = false;
        let mut held = job.lock()?;
        held.facts.running = false;
        match result {
            Ok(outcome) => {
                owner.facts.returned += 1;
                held.facts.returned += 1;
                if outcome == CallbackResult::Declined {
                    owner.facts.declined += 1;
                    held.facts.declined += 1;
                }
                held.waiting_service = job.key.cause != Cause::Allocation;
                held.collect_pending = job.key.cause == Cause::Allocation;
            }
            Err(Error::CaptureActive) => {
                owner.facts.deferred += 1;
                held.owners.push_front(id);
                held.facts.queued = held.owners.len();
            }
            Err(Error::Busy) => {
                // This owner's lock/admission is unavailable. It is not a device-wide
                // capture exclusion and must not starve later owners in the same request.
                owner.facts.returned += 1;
                owner.facts.declined += 1;
                held.facts.returned += 1;
                held.facts.declined += 1;
                held.waiting_service = job.key.cause != Cause::Allocation;
                held.collect_pending = job.key.cause == Cause::Allocation;
            }
            Err(error) => {
                failed(control, &mut owner, error.to_string(), false);
                held.facts.failed += 1;
            }
        }
        drop(held);
        drop(owner);
        control.changed.notify_all();
        #[cfg(all(target_os = "linux", target_has_atomic = "64"))]
        crate::cooperative::wake_cache_actions(control.device)?;
    }
    // Explicit last-owner retirement is separate from no-wait collection. There are no
    // callbacks left to serve. Preserve leases on capture/replay refusal; release them
    // outside every metadata lock and keep exclusion through all last Arena destructors.
    if callbacks.is_empty() && !arenas.is_empty() {
        match crate::enter_retirement(control.device) {
            Ok(guard) => {
                drop(std::mem::take(arenas));
                drop(guard);
            }
            Err(error) if deferred(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
