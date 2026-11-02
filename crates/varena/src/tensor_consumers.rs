//! Native consumer streams for foreign tensor storage backed by a Region Pin.
//!
//! The pointer index holds weak references only. Each foreign storage owns its Pin
//! and a Consumers clone; aliases that share that storage share its deleter. The
//! index lets Torch's allocator hook record interior pointers without owning that
//! storage, taking allocator locks, or calling into Python.

use crate::{Error, Pin, Region, Result};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

static FAMILIES: OnceLock<Mutex<BTreeMap<u64, Vec<Weak<Family>>>>> = OnceLock::new();
// A poisoned index cannot prove which family missed a stream. Existing storage
// then keeps the native context-join fallback, even if its own list is intact.
static TRACKING_FAULT: AtomicBool = AtomicBool::new(false);

struct Family {
    pid: u32,
    region: u64,
    base: u64,
    end: u64,
    reserved_end: u64,
    streams: Mutex<Vec<u64>>,
    active: AtomicBool,
    uncertain: AtomicBool,
}

/// Shared consumer declarations for one native tensor export family.
///
/// This value does not own physical bytes: its one foreign storage separately owns
/// a Pin. Clones share declarations for that same storage and do not create new
/// physical owners. Independent exported storage must register a new family.
#[derive(Clone)]
pub struct Consumers(Arc<Family>);

fn valid_stream(stream: u64) -> Result<()> {
    if stream == 2 {
        return Err(Error::Invalid(
            "per-thread default CUDA streams are unsupported",
        ));
    }
    usize::try_from(stream).map_err(|_| Error::Invalid("CUDA stream handle overflow"))?;
    Ok(())
}

impl Consumers {
    /// Register a live Pin after its producer was ordered on this actual stream.
    /// Registration itself never orders producers or submits CUDA work.
    pub fn new(pin: &Pin, initial_stream: u64) -> Result<Self> {
        crate::check_process()?;
        if initial_stream <= 2 {
            return Err(Error::Invalid(
                "tensor export requires an explicit CUDA consumer stream",
            ));
        }
        valid_stream(initial_stream)?;
        let family = Arc::new(Family {
            pid: std::process::id(),
            region: pin.region().id(),
            base: pin.ptr(),
            end: pin
                .ptr()
                .checked_add(pin.len() as u64)
                .ok_or(Error::Invalid("tensor consumer byte range overflow"))?,
            reserved_end: pin
                .ptr()
                .checked_add(pin.region().allocated_bytes() as u64)
                .ok_or(Error::Invalid("tensor consumer reservation overflow"))?,
            streams: Mutex::new(vec![initial_stream]),
            active: AtomicBool::new(true),
            uncertain: AtomicBool::new(false),
        });
        let mut index = FAMILIES.get_or_init(Mutex::default).lock().map_err(|_| {
            TRACKING_FAULT.store(true, Ordering::Release);
            Error::Poisoned("tensor consumer pointer index".into())
        })?;
        let families = index.entry(family.base).or_default();
        families.retain(|family| family.strong_count() != 0);
        families.push(Arc::downgrade(&family));
        drop(index);
        Ok(Self(family))
    }

    /// Declare another consumer before its storage's final release.
    ///
    /// The caller supplies a live stream in the Pin's primary CUDA context, and
    /// establishes producer/read dependencies before using it. Explicit legacy
    /// default stream handles 0/1 are accepted here; sentinel 2 is not a stable
    /// process-wide handle. A failed declaration keeps conservative reclamation.
    pub fn record(&self, stream: u64) -> Result<()> {
        if !self.0.record_if_active(stream)? {
            return Err(Error::Invalid("tensor consumer storage already released"));
        }
        Ok(())
    }

    /// Record final consumer events and return reservation ownership for deferred
    /// teardown. This never waits for GPU completion or enters Python.
    ///
    /// # Safety
    /// Every consumer through this Pin and its storage aliases has submitted its
    /// work on a declared live stream (or joined it there) and submits nothing
    /// after this call. Call exactly once for this family's one foreign storage,
    /// after its final shared-storage alias is gone. Independent exported storage
    /// retains its own Pin and family. The Pin must belong to this family's
    /// Region. On uncertainty, release keeps the native context-joined fallback.
    #[allow(unsafe_code)]
    pub unsafe fn release(&self, pin: Pin) -> Arc<Region> {
        // Close lookup admission BEFORE releasing physical ownership or allowing
        // canonical Weight retirement to free/reuse the reservation. A transient
        // record_pointer Arc alone must never keep a dead address route active.
        let active = self.0.active.swap(false, Ordering::AcqRel);
        if self.0.pid != std::process::id() {
            return pin.into_unpinned_region();
        }
        self.0.unregister();
        if !active
            || self.0.region != pin.region().id()
            || TRACKING_FAULT.load(Ordering::Acquire)
            || self.0.uncertain.load(Ordering::Acquire)
        {
            return pin.into_unpinned_region();
        }
        let streams = match self.0.streams.lock() {
            Ok(streams) => streams.clone(),
            Err(_) => return pin.into_unpinned_region(),
        };
        // No pointer-index or family mutex crosses native event operations.
        // SAFETY: the caller supplies the final-submission guarantee above.
        unsafe { pin.into_fenced_region(&streams) }
    }
}

/// Route logical Region pointers, including interior views, to all live export
/// families. Unregistered foreign allocations stay the caller's responsibility.
pub(crate) fn record_pointer(ptr: u64, stream: u64) -> Result<bool> {
    crate::check_process()?;
    let Some(index) = FAMILIES.get() else {
        return Ok(false);
    };
    let families: Vec<_> = {
        let index = index.lock().map_err(|_| {
            TRACKING_FAULT.store(true, Ordering::Release);
            Error::Poisoned("tensor consumer pointer index".into())
        })?;
        let Some((_, families)) = index.range(..=ptr).next_back() else {
            return Ok(false);
        };
        families.iter().filter_map(Weak::upgrade).collect()
    };
    // Upgrade under the pointer index, but drop it before stream-list locks or
    // final Arc drops. Family Drop takes the pointer index to remove its key.
    let mut matched = false;
    let mut error = None;
    for family in &families {
        if ptr >= family.reserved_end {
            continue;
        }
        if !family.active.load(Ordering::Acquire) {
            continue;
        }
        let result = if ptr >= family.end {
            Err(Error::Invalid("record_stream outside logical Region bytes"))
        } else {
            family.record_if_active(stream)
        };
        match result {
            Ok(active) => matched |= active,
            Err(failure) => {
                error.get_or_insert(failure);
            }
        }
    }
    if let Some(error) = error {
        // A partial update cannot prove all last-consumer events. Every matched
        // family must retain the conservative native join, including earlier ones.
        for family in &families {
            if ptr < family.reserved_end {
                family.uncertain.store(true, Ordering::Release);
            }
        }
        return Err(error);
    }
    Ok(matched)
}

impl Family {
    fn record_if_active(&self, stream: u64) -> Result<bool> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid("tensor consumers inherited after fork"));
        }
        if !self.active.load(Ordering::Acquire) {
            return Ok(false);
        }
        let result = (|| {
            crate::check_process()?;
            valid_stream(stream)?;
            let mut streams = self
                .streams
                .lock()
                .map_err(|_| Error::Poisoned("tensor consumer streams".into()))?;
            // Release closes admission before taking this same mutex to snapshot
            // stream tails. An admitted declaration is included in that snapshot;
            // a lookup that only retained metadata cannot match reused storage.
            if !self.active.load(Ordering::Acquire) {
                return Ok(false);
            }
            if !streams.contains(&stream) {
                streams.push(stream);
            }
            Ok(true)
        })();
        if result.is_err() {
            self.uncertain.store(true, Ordering::Release);
        }
        result
    }

    fn unregister(&self) {
        if self.pid != std::process::id() {
            return;
        }
        let Some(index) = FAMILIES.get() else {
            return;
        };
        let mut index = match index.lock() {
            Ok(index) => index,
            Err(_) => {
                TRACKING_FAULT.store(true, Ordering::Release);
                return;
            }
        };
        if let Some(families) = index.get_mut(&self.base) {
            // Remove this key before its Region can be reused, even if an in-flight
            // lookup still retains a temporary Arc<Family>. Stale larger-base keys
            // must not hide a newer, larger reservation with a lower base address.
            families.retain(|family| {
                family.as_ptr() != self as *const Family && family.strong_count() != 0
            });
            if families.is_empty() {
                index.remove(&self.base);
            }
        }
    }
}

impl Drop for Family {
    fn drop(&mut self) {
        self.unregister();
    }
}
