//! Same-core foreign-storage ownership for the public Use path.
use super::*;
use crate::tensor_consumers::Consumers;

/// A foreign tensor storage owns this value, independently of its temporary Use.
/// The canonical Weight retains Source and GPU-service custody as well as the Pin.
pub struct TensorExport {
    pin: Option<Pin>,
    consumers: Consumers,
    weight: Option<Arc<Weight>>,
    pid: u32,
}
impl TensorExport {
    pub fn ptr(&self) -> u64 {
        self.pin.as_ref().expect("live tensor export").ptr()
    }
    pub fn len(&self) -> usize {
        self.pin.as_ref().expect("live tensor export").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn device(&self) -> i32 {
        self.pin.as_ref().expect("live tensor export").device()
    }

    /// Declare a later consumer before final storage release. Establish its
    /// producer dependency before reading, and keep the stream live through
    /// release. The final storage deleter records its actual stream tail.
    pub fn record_stream(&self, stream: u64) -> crate::Result<()> {
        self.consumers.record(stream)
    }
}
impl Drop for TensorExport {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            // Foreign capsule owners must not touch inherited native locks or CUDA.
            // The binding leaks the complete holder on fork; retain the Pin even if a
            // native caller drops this value directly in an unsupported child.
            if let Some(pin) = self.pin.take() {
                std::mem::forget(pin);
            }
            if let Some(weight) = self.weight.take() {
                std::mem::forget(weight);
            }
            return;
        }
        if let Some(pin) = self.pin.take() {
            // SAFETY: this value is owned by the foreign storage deleter; its final
            // alias is gone. Every other exported storage owns an independent Pin.
            // Refusal retains the exact Pin and canonical owners. Never turn an
            // unknown tail into a context-wide join or an apparently free region.
            let region = match unsafe { self.consumers.release(pin) } {
                Ok(region) => region,
                Err((pin, error)) => {
                    retirement::quarantine(retirement::Job::Owner(super::Owned::Tensor {
                        _pin: pin,
                        _consumers: self.consumers.clone(),
                        _weight: self.weight.take().expect("tensor Source/service custody"),
                    }), &error);
                    return;
                }
            };
            // Weight still owns this Region and GPU here; this cannot drop the final
            // arena or reservation on the foreign storage deleter's stack.
            drop(region);
        }
        if let Some(weight) = self.weight.take() {
            retirement::defer(retirement::Job::Owner(super::Owned::Weight { _weight: weight }));
        }
    }
}

fn native_error(failure: Failure) -> Error {
    match failure.0 {
        CAPTURING => Error::CaptureActive,
        BUSY => Error::Busy,
        FAULT => Error::Poisoned(failure.1),
        _ => Error::Invalid("invalid public Use handle or stream"),
    }
}

/// Same-cohort read-only event observations, including during a blocked reclaim.
/// These are the existing Driver atomics, not completion or released-byte proof.
/// Do not acquire capture exclusion or the device table: a waiter can hold both.
#[doc(hidden)]
pub fn gpu_event_counts(id: u64) -> crate::Result<(u64, u64, u64)> {
    process().map_err(native_error)?;
    let gpu = {
        let handles = registry_unchecked().map_err(native_error)?;
        let entry = handles
            .get(&id)
            .ok_or(Error::Invalid("invalid public Gpu handle"))?;
        if entry.disposition != Disposition::Owned {
            return Err(Error::Invalid("Gpu ownership has transferred"));
        }
        match &entry.value {
            Handle::Gpu(gpu) => gpu.clone(),
            _ => return Err(Error::Invalid("expected a public Gpu handle")),
        }
    };
    let (_, queries, records, waits) = gpu.core.driver().calls();
    Ok((queries, records, waits))
}

/// Clone a public Use's physical Pin and order its actual Source producer before
/// exposing storage on the named explicit consumer stream. No new fill or Source
/// callback is admitted here; the Use already owns a published or completed fill.
pub fn export_use(id: u64, index: usize, stream: u64) -> crate::Result<TensorExport> {
    let used = usage(id).map_err(native_error)?;
    let weight = used
        ._weights
        .get(index)
        .ok_or(Error::Invalid("tensor export index out of range"))?
        .clone();
    weight.gpu.eager(stream).map_err(native_error)?;
    {
        // Ready Pins have no producer event to order, so order_on alone cannot
        // establish that an override stream belongs to this primary context.
        let _context = weight.gpu.core.driver().enter()?;
        let stream =
            usize::try_from(stream).map_err(|_| Error::Invalid("CUDA stream handle overflow"))?;
        weight.gpu.core.driver().validate_stream(stream)?;
    }
    let pin = {
        let pins = used
            .pins
            .lock()
            .map_err(|_| Error::Poisoned("public Use pins mutex".into()))?;
        pins.as_ref()
            .and_then(|pins| pins.get(index))
            .ok_or(Error::Invalid("Use ended or index out of range"))?
            .try_clone()?
    };
    // SAFETY: eager validated a live explicit stream in the device's primary
    // context. No pointer from this export has been exposed or read yet.
    unsafe {
        pin.order_on(stream)?;
    }
    let consumers = Consumers::new(&pin, stream)?;
    Ok(TensorExport {
        pin: Some(pin),
        consumers,
        weight: Some(weight),
        pid: std::process::id(),
    })
}
