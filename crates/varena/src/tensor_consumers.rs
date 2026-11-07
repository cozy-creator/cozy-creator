//! Native consumer streams for foreign tensor storage backed by a Region Pin.
//!
//! The pointer index holds weak references only. Each foreign storage owns its Pin
//! and a Consumers clone; aliases that share that storage share its deleter. The
//! index lets Torch's allocator hook record interior pointers without owning that
//! storage, taking allocator locks, or calling into Python.

use crate::{Error, Pin, Region, Result, StreamUses};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

static FAMILIES: OnceLock<Mutex<BTreeMap<u64, Vec<Weak<Family>>>>> = OnceLock::new();
// A poisoned index cannot prove which family missed a stream. Existing storage
// then retains its physical Pin as unproven, even if its own list is intact.
static TRACKING_FAULT: AtomicBool = AtomicBool::new(false);

struct Family {
    pid: u32,
    region: u64,
    base: u64,
    end: u64,
    reserved_end: u64,
    streams: StreamUses,
    active: AtomicBool,
    uncertain: AtomicBool,
}

/// Shared consumer declarations for one native tensor export family.
///
/// This value does not own physical bytes: its physical-use owner separately holds
/// a Pin. Storages that structurally share that owner may share this family;
/// independently owned Pins register independent families.
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
        let streams = StreamUses::new(pin.device());
        streams.declare(initial_stream)?;
        Self::from_streams(pin, streams)
    }

    /// Register one independently owned storage with a Use's shared declarations.
    /// Each storage still has its own pointer-route lifetime and physical Pin.
    pub fn from_streams(pin: &Pin, streams: StreamUses) -> Result<Self> {
        crate::check_process()?;
        if streams.device() != pin.device() || !streams.declared()? {
            return Err(Error::Invalid("tensor export requires declared same-device consumers"));
        }
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
            streams,
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
    /// after this call. Call exactly once for this family's physical-use owner,
    /// after every Storage/Use ownership share is gone. Independent physical owners
    /// retain their own Pin and family. The Pin must belong to this family's
    /// Region. Failure returns the exact Pin for explicit unproven custody; the
    /// caller must not drop that Pin or substitute a context synchronization.
    #[allow(unsafe_code)]
    pub unsafe fn release(&self, pin: Pin) -> std::result::Result<Arc<Region>, (Pin, Error)> {
        // Close lookup admission BEFORE releasing physical ownership or allowing
        // canonical Weight retirement to free/reuse the reservation. A transient
        // record_pointer Arc alone must never keep a dead address route active.
        let active = self.0.active.swap(false, Ordering::AcqRel);
        if self.0.pid != std::process::id() {
            return Err((pin, Error::Invalid("tensor consumer inherited after fork")));
        }
        self.0.unregister();
        if !active
            || self.0.region != pin.region().id()
            || TRACKING_FAULT.load(Ordering::Acquire)
            || self.0.uncertain.load(Ordering::Acquire)
        {
            return Err((pin, Error::Invalid("tensor consumer completion is unproven")));
        }
        // SAFETY: the caller closes final submission. Shared epoch gates serialize
        // fence installation with checked retirement, including delayed storage GC.
        unsafe { self.0.streams.release_pin(pin) }
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
        // family must retain its physical Pin, including earlier ones.
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
            self.streams.declare_if(stream, || self.active.load(Ordering::Acquire))
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
