//! Owned producer submission, event ordering and deferred native fill completion.
use platform::signal::{Signal, wait_any};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::time::Duration;

use crate::{
    Contents, Error, Fill, Pin, Preparation, ProducerGuard, Region, Result, gate,
    enter_producer, enter_retirement,
};

/// Outcome of checked native fill-resource retirement. Neither variant grants capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRetirement {
    Complete,
    Pending,
}

/// An actual producer resource whose cleanup can be checked independently of GPU completion.
///
/// # Safety
/// retire runs only after the fixed producers are checked complete and under retirement
/// exclusion. It must not wait for Python/the GIL. Complete leaves this wrapper inert or
/// transfers every owner with equivalent completion/exclusion custody. Pending or Err must
/// retain every unreleased owner. Err is permanent: the core quarantines instead of retrying.
#[allow(unsafe_code)]
pub unsafe trait FillResource: Send + 'static {
    fn retire(&mut self) -> Result<FillRetirement>;
}

/// Native resources retained until every fixed producer is checked complete.
pub struct FillOwners {
    streams: Vec<u64>,
    resources: Vec<Box<dyn FillResource>>,
}
impl FillOwners {
    /// # Safety
    /// All streams are live in the fill device's primary context. Resources retain those
    /// streams, direct inputs, staging and libraries. Their checked retirement obeys
    /// FillResource's ownership/exclusion contract. No unlisted writer exists.
    #[allow(unsafe_code)]
    pub unsafe fn new(streams: Vec<u64>, resources: Vec<Box<dyn FillResource>>) -> Self {
        Self { streams, resources }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillState {
    Prepared,
    Writing,
    Submitted,
    Published,
    Completed,
    Quarantined,
}

#[derive(Clone, Debug)]
pub struct FillFacts {
    pub state: FillState,
    pub submission_closed: bool,
    pub published: bool,
    pub completed: bool,
    /// Native payload retirement finished; not a physical-release/capacity certificate.
    pub retirement_finished: bool,
    pub producer_count: usize,
    pub recorded_count: usize,
    pub completed_count: usize,
    pub failure: Option<Error>,
    pub retirement_failure: Option<Error>,
    pub resources_remaining: usize,
}
struct Observed {
    facts: FillFacts,
    events: Vec<usize>, // identities only; Payload is the sole CUDA owner
    readiness: Vec<Weak<Signal>>,
}
impl Observed {
    fn wake_readiness(&mut self) {
        self.readiness.retain(|waiter| {
            if let Some(signal) = waiter.upgrade() {
                signal.set();
                true
            } else {
                false
            }
        });
    }
}
pub(crate) struct Control {
    pid: u32,
    pub(crate) generation: u64,
    observed: Mutex<Observed>,
}
impl Control {
    fn check(&self) -> Result<()> {
        if self.pid != std::process::id() {
            Err(Error::Invalid(
                "asynchronous fills cannot be used after fork",
            ))
        } else {
            Ok(())
        }
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Observed>> {
        self.check()?;
        self.observed
            .lock()
            .map_err(|_| Error::Poisoned("fill ticket mutex".into()))
    }
    fn facts(&self) -> Result<FillFacts> {
        Ok(self.lock()?.facts.clone())
    }

    fn capture_ready(&self) -> Result<bool> {
        let observed = self.lock()?;
        if let Some(error) = observed
            .facts
            .retirement_failure
            .as_ref()
            .or(observed.facts.failure.as_ref())
        {
            return Err(error.clone());
        }
        if observed.facts.state == FillState::Quarantined {
            return Err(Error::Poisoned(
                "fill capture readiness is quarantined".into(),
            ));
        }
        Ok(observed.facts.completed && observed.facts.retirement_finished)
    }

    fn watch_readiness(&self, signal: &Arc<Signal>) -> Result<()> {
        let mut observed = self.lock()?;
        observed
            .readiness
            .retain(|waiter| waiter.strong_count() != 0);
        observed.readiness.push(Arc::downgrade(signal));
        Ok(())
    }
}

// The caller retains the public Use's pins mutex and its retirement exclusion.
// No registry/capture/arena/submission/observed mutex crosses the platform wait.
pub(crate) fn wait_use_ready(pins: &[Pin]) -> Result<()> {
    fn submission(pin: &Pin) -> Result<(u64, Option<Arc<Control>>)> {
        crate::check_process()?;
        let region = pin.region();
        if region.pid != std::process::id() {
            return Err(Error::Invalid("capture readiness inherited after fork"));
        }
        let generation = region.uses.generation.load(Ordering::Acquire);
        let control = region
            .uses
            .submission
            .lock()
            .map_err(|_| Error::Poisoned("fill submission mutex".into()))?
            .clone();
        if generation != region.uses.generation.load(Ordering::Acquire)
            || control
                .as_ref()
                .is_some_and(|control| control.generation != generation)
            || pin.submission.as_ref().is_some_and(|ordered| {
                ordered.generation != generation
                    || !control
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(ordered, current))
            })
        {
            return Err(Error::Invalid("capture readiness fill generation changed"));
        }
        Ok((generation, control))
    }
    let snapshots = pins.iter().map(submission).collect::<Result<Vec<_>>>()?;
    let mut controls = Vec::new();
    for (_, control) in &snapshots {
        if let Some(control) = control
            && !controls.iter().any(|known| Arc::ptr_eq(known, control))
        {
            controls.push(control.clone());
        }
    }
    let ready = || -> Result<bool> {
        let mut complete = true;
        for control in &controls {
            complete &= control.capture_ready()?;
        }
        if !complete {
            return Ok(false);
        }
        for (pin, (generation, original)) in pins.iter().zip(&snapshots) {
            let _state = pin.region().gpu.lock()?;
            let (current_generation, current) = submission(pin)?;
            let same = match (original, current.as_ref()) {
                (Some(original), Some(current)) => Arc::ptr_eq(original, current),
                (None, None) => true,
                _ => false,
            };
            if generation != &current_generation
                || !same
                || pin.region().uses.state() != Contents::Ready
            {
                return Err(Error::Invalid("capture readiness contents changed"));
            }
        }
        Ok(true)
    };
    if ready()? {
        return Ok(());
    }
    let signal = Arc::new(
        Signal::new()
            .map_err(|error| Error::Unsupported(format!("capture readiness wake: {error}")))?,
    );
    for control in &controls {
        control.watch_readiness(&signal)?;
    }
    loop {
        // Clear before reading facts: a transition is either visible in those
        // facts or leaves this invocation's own signal set for wait_any.
        signal.clear();
        if ready()? {
            return Ok(());
        }
        let events = wait_any(&[signal.waitable()])
            .map_err(|error| Error::Unsupported(format!("capture readiness wait: {error}")))?;
        if events.iter().any(|event| event.broken) {
            return Err(Error::Unsupported("capture readiness wake closed".into()));
        }
    }
}

/// Metadata-only observer. Dropping it neither cancels work nor releases CUDA resources.
pub struct FillTicket {
    control: Arc<Control>,
}
impl FillTicket {
    pub fn facts(&self) -> Result<FillFacts> {
        self.control.facts()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FillDestination {
    pub device: i32,
    pub ptr: u64,
    pub bytes: usize,
}

/// Unique producer-side custody. Refusals never consume this owner.
pub struct FillSession {
    control: Arc<Control>,
    payload: Option<Payload>,
    sender: Option<mpsc::Sender<Payload>>,
}

impl Fill {
    /// Transfer custody without CUDA work. Call begin before exposing any destination.
    ///
    /// # Safety
    /// The Fill has no earlier unaccounted producer. All future writers use the fixed
    /// streams/resources and stop host submission before publish/abort. Escaped views own Pins.
    #[allow(unsafe_code)]
    pub unsafe fn into_async(self, owners: FillOwners) -> FillSession {
        let control = Arc::new(Control {
            pid: self.region().pid,
            generation: self.region().uses.generation.load(Ordering::Acquire),
            observed: Mutex::new(Observed {
                facts: FillFacts {
                    state: FillState::Prepared,
                    submission_closed: false,
                    published: false,
                    completed: false,
                    retirement_finished: false,
                    producer_count: owners.streams.len(),
                    recorded_count: 0,
                    completed_count: 0,
                    failure: None,
                    retirement_failure: None,
                    resources_remaining: owners.resources.len(),
                },
                events: Vec::new(),
                readiness: Vec::new(),
            }),
        });
        let region = Arc::clone(self.pin.as_ref().expect("fill pin").region());
        FillSession {
            control: Arc::clone(&control),
            sender: None,
            payload: Some(Payload {
                control,
                region: Some(region),
                fill: Some(self),
                completed_pin: None,
                owners: Some(owners),
                guard: None,
                events: Vec::new(),
                checked: Vec::new(),
                writing: false,
                sealed: false,
                publish: false,
                completed: false,
            }),
        }
    }
}

impl FillSession {
    pub fn facts(&self) -> Result<FillFacts> {
        self.control.facts()
    }

    pub fn begin(&mut self) -> Result<FillDestination> {
        let result = self.begin_inner();
        if let Err(error) = &result {
            if let Ok(mut observed) = self.control.lock() {
                observed.facts.failure = Some(error.clone());
            }
        }
        result
    }

    fn begin_inner(&mut self) -> Result<FillDestination> {
        self.control.check()?;
        let payload = self
            .payload
            .as_mut()
            .ok_or(Error::Invalid("fill session was submitted"))?;
        if payload.writing
            || payload.sealed
            || self.control.facts()?.state == FillState::Quarantined
        {
            return Err(Error::Invalid("fill session is not prepared"));
        }
        // Startup cannot lose writes: no destination has been exposed yet.
        self.sender = Some(worker(payload.region().device())?);
        let owners = payload.owners.as_ref().expect("session owners");
        if owners.streams.is_empty() {
            return Err(Error::Invalid(
                "asynchronous fill requires producer streams",
            ));
        }
        if owners
            .streams
            .iter()
            .enumerate()
            .any(|(i, stream)| owners.streams[..i].contains(stream))
        {
            return Err(Error::Invalid("duplicate asynchronous producer stream"));
        }
        let region = payload.region();
        let guard = enter_producer(region.device())?;
        let result = (|| {
            let _arena = region.gpu.lock()?;
            if region.uses.state() != Contents::Loading
                || region.uses.generation.load(Ordering::Acquire) != self.control.generation
            {
                return Err(Error::Invalid(
                    "asynchronous fill generation changed before begin",
                ));
            }
            let _context = region.gpu.driver().enter()?;
            for &stream in &owners.streams {
                region.gpu.driver().validate_stream(stream as usize)?;
                if region.gpu.driver().is_capturing(stream as usize)? {
                    return Err(Error::CaptureActive);
                }
            }
            // A failed pre-write attempt keeps every already-created event for eventual cleanup.
            while payload.events.len() < owners.streams.len() {
                payload.events.push(region.gpu.driver().create_event()?);
                payload.checked.push(false);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.control.lock()?.facts.failure = Some(error.clone());
            return Err(error);
        }
        let destination = FillDestination {
            device: region.device(),
            ptr: region.ptr(),
            bytes: region.len(),
        };
        payload.guard = Some(guard);
        payload.writing = true;
        let mut observed = self.control.lock()?;
        observed.facts.state = FillState::Writing;
        observed.facts.failure = None;
        observed.events.clone_from(&payload.events);
        Ok(destination)
    }

    /// # Safety
    /// Every byte has a successfully enqueued initialization; no host thread may submit any
    /// further write. Every actual GPU writer is one of the fixed, retained producer streams.
    #[allow(unsafe_code)]
    pub unsafe fn publish(&mut self) -> Result<FillTicket> {
        self.seal(true, None, None).map(|(ticket, _)| ticket)
    }

    /// As `publish`, with the consumer's pin taken and ordered under the same lock, so nothing
    /// can evict the region between the two.
    ///
    /// # Safety
    /// As `publish`; the stream is live in this primary context.
    #[allow(unsafe_code)]
    pub unsafe fn publish_to(&mut self, consumer_stream: u64) -> Result<Pin> {
        self.seal(true, None, Some(consumer_stream)).map(|(_, pin)| pin.expect("published pin"))
    }

    /// # Safety
    /// No further host submission may occur. Partial writes used only the fixed producer set.
    #[allow(unsafe_code)]
    pub unsafe fn abort(&mut self, cause: Error) -> Result<FillTicket> {
        self.seal(false, Some(cause), None).map(|(ticket, _)| ticket)
    }

    fn seal(&mut self, publish: bool, cause: Option<Error>, consumer: Option<u64>) -> Result<(FillTicket, Option<Pin>)> {
        self.control.check()?;
        let payload = self
            .payload
            .as_mut()
            .ok_or(Error::Invalid("fill session was submitted"))?;
        if !payload.writing
            || payload.sealed
            || self.control.facts()?.state == FillState::Quarantined
        {
            return Err(Error::Invalid("fill session is not writing"));
        }
        let region = payload.region();
        let result = (|| {
            let gate = gate(region.device())?;
            gate.not_capturing()?;
            let mut state = region.gpu.lock()?;
            let _context = region.gpu.driver().enter()?;
            let owners = payload.owners.as_ref().expect("session owners");
            // A refusal before recording leaves a reachable Writing session for retry/abort.
            for &stream in &owners.streams {
                if region.gpu.driver().is_capturing(stream as usize)? {
                    return Err(Error::CaptureActive);
                }
            }
            let mut observed = self.control.lock()?;
            observed.facts.submission_closed = true;
            observed.facts.state = FillState::Submitted;
            observed.facts.failure = cause.clone();
            payload.sealed = true;
            for (&stream, &event) in owners.streams.iter().zip(&payload.events) {
                region.gpu.driver().record(event, stream as usize)?;
                observed.facts.recorded_count += 1;
            }
            if region.uses.generation.load(Ordering::Acquire) != self.control.generation
                || region.uses.state() != Contents::Loading
            {
                return Err(Error::Invalid("asynchronous fill generation changed"));
            }
            payload.publish = publish;
            if !publish {
                return Ok(None);
            }
            *region
                .uses
                .submission
                .lock()
                .map_err(|_| Error::Poisoned("fill submission mutex".into()))? =
                Some(Arc::clone(&self.control));
            region.uses.set(Contents::Published);
            observed.facts.published = true;
            observed.facts.state = FillState::Published;
            drop(observed);
            let Some(consumer) = consumer else { return Ok(None) };
            order(&region, &self.control, consumer)?;
            let mut pin = region.pin_locked(&mut state)?;
            pin.submission = Some(Arc::clone(&self.control));
            // Keep the table lock through publication, but no resource destruction here.
            Ok(Some(pin))
        })();
        let pin = match result {
            Ok(pin) => pin,
            Err(error) => {
                if payload.sealed {
                    payload.quarantine(error.clone());
                } else {
                    self.control.lock()?.facts.failure = Some(error.clone());
                }
                return Err(error);
            }
        };
        let payload = self.payload.take().expect("submitted payload");
        if let Err(error) = self
            .sender
            .as_ref()
            .expect("worker before writes")
            .send(payload)
        {
            let mut payload = error.0;
            let error = Error::Poisoned("fill completion worker stopped".into());
            payload.quarantine(error.clone());
            self.payload = Some(payload); // publication did not release custody
            std::mem::forget(pin);
            return Err(error);
        }
        Ok((FillTicket { control: Arc::clone(&self.control) }, pin))
    }
}

impl Drop for FillSession {
    fn drop(&mut self) {
        let Some(mut payload) = self.payload.take() else {
            return;
        };
        if self.control.pid != std::process::id() {
            std::mem::forget(payload); // no inherited mutex, CUDA, resource or driver cleanup
            return;
        }
        if payload.writing {
            // No seal means no proof that another host submitter has stopped. Even a sealed
            // failure retains all actual owners; no retry can silently turn it into Ready.
            payload.quarantine(Error::Poisoned(
                "unproven asynchronous fill abandoned".into(),
            ));
            std::mem::forget(payload);
            return;
        }
        // Prepared never exposed a destination. Deferred cleanup is allowed to prove that
        // there was no submitted work; failure to queue keeps the actual owners quarantined.
        let device = payload.region().device();
        match self.sender.take().map(Ok).unwrap_or_else(|| worker(device)) {
            Ok(sender) => {
                if let Err(error) = sender.send(payload) {
                    let mut payload = error.0;
                    payload.quarantine(Error::Poisoned(
                        "prepared fill cleanup worker stopped".into(),
                    ));
                    std::mem::forget(payload);
                }
            }
            Err(error) => {
                payload.quarantine(error);
                std::mem::forget(payload);
            }
        }
    }
}

pub(crate) struct Payload {
    control: Arc<Control>,
    region: Option<Arc<Region>>,
    fill: Option<Fill>,
    completed_pin: Option<Pin>,
    owners: Option<FillOwners>,
    guard: Option<ProducerGuard>,
    events: Vec<usize>,
    checked: Vec<bool>,
    writing: bool,
    sealed: bool,
    publish: bool,
    completed: bool,
}
impl Payload {
    fn region(&self) -> Arc<Region> {
        Arc::clone(self.region.as_ref().expect("worker region custody"))
    }
    fn quarantine(&mut self, error: Error) {
        self.quarantine_at(error, false);
    }
    pub(crate) fn retirement_failed(&mut self, error: &Error) {
        self.quarantine_at(error.clone(), true);
    }
    fn quarantine_at(&mut self, error: Error, retirement: bool) {
        // Called only after every operation guard has left the stack.
        let phase = if retirement {
            "resource retirement"
        } else {
            "producer"
        };
        self.region()
            .poison(format!("asynchronous fill {phase}: {error}"));
        crate::abi::fault();
        if self.control.check().is_ok() {
            // A worker panic may poison the observer while a waiter is asleep.
            // Publish/wake under the recovered guard, leaving poison intact so
            // readers still return an error rather than hanging or claiming ready.
            let mut observed = self
                .control
                .observed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            observed.facts.state = FillState::Quarantined;
            if retirement {
                observed.facts.retirement_failure = Some(error);
            } else {
                observed.facts.failure = Some(error);
            }
            observed.wake_readiness();
        }
    }
    fn tick(&mut self) -> Result<bool> {
        self.control.check()?;
        let region = self.region(); // lease is acquired before capture exclusion and outlives it
        if !self.completed {
            let gate = gate(region.device())?;
            if gate.not_capturing().is_err() {
                return Ok(false);
            }
            let mut arena = region.gpu.lock()?;
            let _context = region.gpu.driver().enter()?;
            let mut observed = self.control.lock()?;
            if self.writing {
                if !self.sealed || observed.facts.recorded_count != self.events.len() {
                    return Err(Error::Poisoned(
                        "unsealed fill reached completion worker".into(),
                    ));
                }
                for (index, &event) in self.events.iter().enumerate() {
                    if !self.checked[index] && region.gpu.driver().query(event)? {
                        self.checked[index] = true;
                        observed.facts.completed_count += 1;
                    }
                }
                if observed.facts.completed_count != self.events.len() {
                    return Ok(false);
                }
            }
            if region.uses.generation.load(Ordering::Acquire) != self.control.generation {
                return Err(Error::Poisoned("fill completion generation changed".into()));
            }
            let mut fill = self.fill.take().expect("worker fill");
            self.completed_pin = fill.pin.take(); // empty Fill drop cannot reset Published/Ready
            if self.publish {
                region.uses.set(Contents::Ready);
                arena.fills = arena.fills.saturating_add(1);
                arena.filled_bytes = arena.filled_bytes.saturating_add(region.len() as u64);
            } else {
                region.uses.set(Contents::Cold);
            }
            self.completed = true;
            observed.facts.completed = true;
            observed.facts.state = FillState::Completed;
        }
        Ok(true)
    }
    pub(crate) fn retire(&mut self) -> Result<bool> {
        self.control.check()?;
        if !self.completed {
            return Err(Error::Poisoned("fill retired before completion".into()));
        }
        let region = self.region();
        // All final GPU-owning drops are outside the capture/arena/control mutexes. The
        // retirement token excludes new capture/replay without retaining the capture lock.
        let _retirement = match enter_retirement(region.device()) {
            Ok(guard) => guard,
            Err(Error::CaptureActive) => return Ok(false),
            Err(error) => return Err(error),
        };
        let _context = region.gpu.driver().enter()?;
        while let Some(event) = self.events.last().copied() {
            region.gpu.driver().destroy_event(event)?;
            self.events.pop();
        }
        let resources = &mut self.owners.as_mut().expect("retirement owners").resources;
        let mut index = 0;
        while index < resources.len() {
            match resources[index].retire()? {
                FillRetirement::Complete => {
                    // Safety contract: the successful wrapper is now inert (or custody was
                    // transferred), so its destructor cannot release an unproved resource.
                    let inert = resources.remove(index);
                    self.control.lock()?.facts.resources_remaining = resources.len();
                    drop(inert);
                }
                FillRetirement::Pending => index += 1,
            }
        }
        if !resources.is_empty() {
            return Ok(false);
        }
        drop(self.owners.take());
        if let Some(pin) = self.completed_pin.take() {
            // SAFETY: every fixed producer is checked complete (or begin never exposed a
            // destination). Each escaped/ordered consumer retains its own separate Pin.
            #[allow(unsafe_code)]
            drop(unsafe { pin.into_completed_region() });
        }
        if !self.publish {
            let _ = region.release_unready()?;
        }
        drop(self.guard.take());
        drop(_context);
        drop(self.region.take());
        drop(region); // must precede retirement exclusion's release, even for the last Region
        drop(_retirement);
        // There are no GPU owners left to quarantine if metadata was poisoned. Preserve
        // that poison for observers, but never send this empty payload through failure cleanup.
        let mut observed = self
            .control
            .observed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        observed.facts.retirement_finished = true;
        observed.wake_readiness();
        Ok(true)
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        // Error/panic fallback never executes GPU-owning destructors on an arbitrary stack.
        if let Some(fill) = self.fill.take() {
            std::mem::forget(fill);
        }
        if let Some(pin) = self.completed_pin.take() {
            std::mem::forget(pin);
        }
        if let Some(owners) = self.owners.take() {
            std::mem::forget(owners);
        }
        if let Some(guard) = self.guard.take() {
            std::mem::forget(guard);
        }
        if let Some(region) = self.region.take() {
            std::mem::forget(region);
        }
    }
}

static PID: AtomicU32 = AtomicU32::new(0);
static WORKER: OnceLock<Mutex<BTreeMap<i32, mpsc::Sender<Payload>>>> = OnceLock::new();
fn worker(device: i32) -> Result<mpsc::Sender<Payload>> {
    let pid = std::process::id();
    let previous = PID
        .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
        .unwrap_or_else(|v| v);
    if previous != 0 && previous != pid {
        return Err(Error::Invalid("fill worker inherited after fork"));
    }
    // Both event polling and final-resource custody exist before any destination is exposed.
    crate::retirement::prepare()?;
    let mut slot = WORKER
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned("fill worker registry mutex".into()))?;
    if let Some(sender) = slot.get(&device) {
        return Ok(sender.clone());
    }
    let (sender, receiver) = mpsc::channel::<Payload>();
    std::thread::Builder::new()
        .name(format!("varena-fill-{device}"))
        .spawn(move || run_worker(receiver))
        .map_err(|error| {
            Error::Unsupported(format!("fill completion worker could not start: {error}"))
        })?;
    slot.insert(device, sender.clone());
    Ok(sender)
}

fn run_worker(receiver: mpsc::Receiver<Payload>) {
    let mut pending = Vec::new();
    loop {
        let received = if pending.is_empty() {
            receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            // A fill ends on the GPU, which wakes no host thread short of a host function holding each producer stream.
            receiver.recv_timeout(Duration::from_millis(1))
        };
        match received {
            Ok(payload) => pending.push(payload),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) if pending.is_empty() => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        pending.extend(receiver.try_iter());
        let mut index = 0;
        while index < pending.len() {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pending[index].tick()));
            match result {
                Ok(Ok(false)) => index += 1,
                Ok(Ok(true)) => {
                    let payload = pending.swap_remove(index);
                    // The retirer owns every checked final resource.
                    crate::retirement::defer(crate::retirement::Job::Fill(payload));
                }
                failure => {
                    let error = match failure {
                        Ok(Err(error)) => error,
                        _ => Error::Poisoned("fill completion worker panicked".into()),
                    };
                    let mut payload = pending.swap_remove(index);
                    payload.quarantine(error);
                    std::mem::forget(payload);
                }
            }
        }
    }
}

pub(crate) enum StreamPreparation {
    Ready(Pin),
    Ordered(Pin),
    Fill(Fill),
}
impl Region {
    /// Demand admission with explicit producer ordering.
    ///
    /// # Safety
    /// Consumer stream is live in this primary context. No dependent reads may be enqueued
    /// before success; every additional consumer stream must call Pin::order_on first.
    #[allow(unsafe_code)]
    pub(crate) unsafe fn prepare_on(self: &Arc<Self>, consumer_stream: u64) -> Result<StreamPreparation> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid("CUDA regions cannot be used after fork"));
        }
        {
            let gate = gate(self.device())?;
            let mut state = self.gpu.lock()?;
            if self.uses.state() == Contents::Published {
                gate.not_capturing()?;
                let control = self
                    .uses
                    .submission
                    .lock()
                    .map_err(|_| Error::Poisoned("fill submission mutex".into()))?
                    .clone()
                    .ok_or(Error::Poisoned("published fill has no ticket".into()))?;
                order(self, &control, consumer_stream)?;
                state.used(self.id, self.gpu.next_use());
                let mut pin = self.pin_locked(&mut state)?;
                pin.submission = Some(control);
                return Ok(StreamPreparation::Ordered(pin));
            }
        }
        self.prepare().map(|value| match value {
            Preparation::Resident(pin) => StreamPreparation::Ready(pin),
            Preparation::Fill(fill) => StreamPreparation::Fill(fill),
        })
    }
}
impl Pin {
    /// # Safety
    /// Stream is live in this primary context; call before any additional stream reads.
    #[allow(unsafe_code)]
    pub unsafe fn order_on(&self, consumer_stream: u64) -> Result<()> {
        let region = self.region();
        if region.pid != std::process::id() {
            return Err(Error::Invalid("CUDA regions cannot be used after fork"));
        }
        let gate = gate(region.device())?;
        let _state = region.gpu.lock()?;
        if let Some(control) = &self.submission {
            if !control.lock()?.facts.completed {
                gate.not_capturing()?;
            }
            order(region, control, consumer_stream)
        } else {
            Ok(())
        }
    }
}
// Caller owns capture exclusion and the arena mutex. Control has no GPU-owning destructor.
fn order(region: &Region, control: &Control, stream: u64) -> Result<()> {
    let observed = control.lock()?;
    if control.generation != region.uses.generation.load(Ordering::Acquire) {
        return Err(Error::Invalid("ordered pin fill generation changed"));
    }
    if observed.facts.state == FillState::Quarantined {
        return Err(observed
            .facts
            .failure
            .clone()
            .or_else(|| observed.facts.retirement_failure.clone())
            .unwrap_or(Error::Poisoned("fill quarantined".into())));
    }
    if !observed.facts.published {
        return Err(Error::Invalid("fill was not published"));
    }
    if observed.facts.completed {
        return Ok(());
    }
    let _context = region.gpu.driver().enter()?;
    region.gpu.driver().validate_stream(stream as usize)?;
    if region.gpu.driver().is_capturing(stream as usize)? {
        return Err(Error::CaptureActive);
    }
    if observed.facts.recorded_count != observed.events.len() {
        return Err(Error::Poisoned(
            "producer events were not all recorded".into(),
        ));
    }
    for &event in &observed.events {
        region.gpu.driver().order_event(stream as usize, event)?;
    }
    Ok(())
}
