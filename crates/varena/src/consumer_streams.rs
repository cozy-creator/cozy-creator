//! Shared consumer-stream epochs. Completion witnesses survive raw-handle reuse.
use crate::{Error, Pin, Region, Result};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Key = (i32, u64);
static CURRENT: OnceLock<Mutex<BTreeMap<Key, Weak<Epoch>>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Epoch {
    key: Key,
    id: u64,
    pid: u32,
    complete: AtomicBool,
    gate: Mutex<()>,
}

impl Drop for Epoch {
    fn drop(&mut self) {
        if self.pid != std::process::id() { return; }
        let Some(registry) = CURRENT.get() else { return; };
        if let Ok(mut registry) = registry.lock() {
            if registry.get(&self.key).is_some_and(|entry| entry.as_ptr() == self as *const Epoch) {
                registry.remove(&self.key);
            }
        }
    }
}

fn epoch(device: i32, stream: u64) -> Result<Arc<Epoch>> {
    crate::check_process()?;
    let mut previous = None;
    let result = {
        let mut registry = CURRENT.get_or_init(Mutex::default).lock()
            .map_err(|_| Error::Poisoned("consumer stream registry".into()))?;
        if let Some(value) = registry.get(&(device, stream)).and_then(Weak::upgrade) {
            if !value.complete.load(Ordering::Acquire) { return Ok(value); }
            previous = Some(value); // Never drop a final Epoch while its registry is locked.
        }
        let id = NEXT.try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| Error::Invalid("consumer stream epoch overflow"))?;
        let value = Arc::new(Epoch { key: (device, stream), id, pid: std::process::id(),
            complete: AtomicBool::new(false), gate: Mutex::new(()) });
        registry.insert(value.key, Arc::downgrade(&value));
        value
    };
    drop(previous);
    Ok(result)
}

pub(crate) fn tracked(device: i32, stream: u64) -> Result<bool> {
    crate::check_process()?;
    let registry = CURRENT.get_or_init(Mutex::default).lock()
        .map_err(|_| Error::Poisoned("consumer stream registry".into()))?;
    Ok(registry.get(&(device, stream)).is_some_and(|entry| entry.strong_count() != 0))
}

/// The caller has stopped submissions and retains the stream until this returns.
/// The check must prove the still-live stream drained; failure leaves the epoch active.
pub(crate) fn retire(device: i32, stream: u64, check: impl FnOnce() -> Result<()>) -> Result<()> {
    let value = epoch(device, stream)?;
    {
        let _gate = value.gate.lock().map_err(|_| Error::Poisoned("consumer stream epoch".into()))?;
        if value.complete.load(Ordering::Acquire) { return Ok(()); }
        check()?;
        let mut registry = CURRENT.get_or_init(Mutex::default).lock()
            .map_err(|_| Error::Poisoned("consumer stream registry".into()))?;
        // Nothing fallible follows publication. A refusal leaves the witness active.
        value.complete.store(true, Ordering::Release);
        if registry.get(&value.key).is_some_and(|entry| entry.as_ptr() == Arc::as_ptr(&value)) {
            registry.remove(&value.key);
        }
    }
    Ok(())
}

#[derive(Default)]
struct Uses {
    declared: bool,
    epochs: Vec<Arc<Epoch>>,
}

struct Family {
    device: i32,
    pid: u32,
    uncertain: AtomicBool,
    uses: Mutex<Uses>,
}

/// Metadata shared by a Use and its independently owned storage exports.
/// It owns neither physical Pins nor CUDA streams.
#[derive(Clone)]
pub struct StreamUses(Arc<Family>);

impl StreamUses {
    pub fn new(device: i32) -> Self {
        Self(Arc::new(Family { device, pid: std::process::id(),
            uncertain: AtomicBool::new(false), uses: Mutex::new(Uses::default()) }))
    }
    pub fn device(&self) -> i32 { self.0.device }
    pub fn declared(&self) -> Result<bool> {
        crate::check_process()?;
        if self.0.pid != std::process::id() { return Err(Error::Invalid("consumer streams inherited after fork")); }
        self.0.uses.lock().map(|uses| uses.declared)
            .map_err(|_| Error::Poisoned("consumer stream family".into()))
    }

    pub fn declare(&self, stream: u64) -> Result<()> {
        self.declare_if(stream, || true).map(|_| ())
    }

    pub(crate) fn declare_if(&self, stream: u64, admitted: impl FnOnce() -> bool) -> Result<bool> {
        let result = (|| {
            crate::check_process()?;
            if self.0.pid != std::process::id() { return Err(Error::Invalid("consumer streams inherited after fork")); }
            if stream == 2 { return Err(Error::Invalid("per-thread default CUDA streams are unsupported")); }
            usize::try_from(stream).map_err(|_| Error::Invalid("CUDA stream handle overflow"))?;
            let value = epoch(self.0.device, stream)?;
            let mut uses = self.0.uses.lock().map_err(|_| Error::Poisoned("consumer stream family".into()))?;
            if !admitted() { return Ok(false); }
            uses.epochs.retain(|epoch| !epoch.complete.load(Ordering::Acquire));
            if !uses.epochs.iter().any(|epoch| Arc::ptr_eq(epoch, &value)) { uses.epochs.push(value); }
            uses.declared = true;
            Ok(true)
        })();
        if result.is_err() { self.0.uncertain.store(true, Ordering::Release); }
        result
    }

    /// Close one physical Pin after all its aliases stopped submitting. Other Pins
    /// sharing this Use may declare later consumers independently.
    ///
    /// # Safety
    /// Every consumer through this Pin was declared before submission. Stream owners
    /// call checked retirement before destruction and stop submissions during that call.
    #[allow(unsafe_code)]
    pub unsafe fn release_pin(&self, pin: Pin) -> std::result::Result<Arc<Region>, (Pin, Error)> {
        let mut pin = Some(pin);
        let result = (|| {
            crate::check_process()?;
            if self.0.pid != std::process::id() || self.0.uncertain.load(Ordering::Acquire)
                || self.0.device != pin.as_ref().expect("owned Pin").device() {
                return Err(Error::Invalid("consumer stream completion is unproven"));
            }
            let uses = self.0.uses.lock().map_err(|_| Error::Poisoned("consumer stream family".into()))?;
            if !uses.declared { return Err(Error::Invalid("no consumer stream was declared")); }
            let mut epochs = uses.epochs.clone();
            epochs.sort_by_key(|epoch| epoch.id);
            let guards = epochs.iter().map(|epoch| epoch.gate.lock()
                .map_err(|_| Error::Poisoned("consumer stream epoch".into())))
                .collect::<Result<Vec<_>>>()?;
            let streams = epochs.iter().filter(|epoch| !epoch.complete.load(Ordering::Acquire))
                .map(|epoch| epoch.key.1).collect::<Vec<_>>();
            let held = pin.take().expect("owned Pin");
            let result = if streams.is_empty() {
                // SAFETY: every declared epoch was checked complete before its raw stream
                // could be destroyed. This final alias can no longer submit through the Pin.
                Ok(unsafe { held.into_completed_region() })
            } else {
                // SAFETY: epoch gates exclude stream retirement/destruction until all last
                // consumer fences are installed; final-submission ownership is the caller's.
                unsafe { held.try_into_fenced_region(&streams) }
            };
            drop(guards);
            match result {
                Ok(region) => Ok(region),
                Err((held, error)) => { pin = Some(held); Err(error) }
            }
        })();
        result.map_err(|error| (pin.expect("refused Pin retained"), error))
    }
}
