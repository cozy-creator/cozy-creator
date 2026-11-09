//! Advanced interop shared by language bindings and placement integrations.
//! Ordinary Rust clients drive async work on their own runtime.

use std::sync::Arc;

use crate::{Failure, Result};

#[cfg(feature = "cuda")]
pub use crate::io::decode::{DeviceDecode, Output as DecodeOutput, Part as DecodePart};
pub use crate::session::PartData;

/// Check encoded-part descriptors through the shared format registry without reading data.
#[cfg(feature = "cuda")]
pub fn check_decode(
    encoding: &crate::format::Encoding,
    dtype: &crate::format::Dtype,
    shape: &[u64],
    parts: &[crate::format::decode::PartMetadata<'_>],
) -> Result<()> {
    crate::io::decode::check(encoding, dtype, shape, parts)
}

/// Decode existing part buffers into an existing, exactly typed destination.
///
/// # Safety
/// Every span must satisfy [`crate::io::decode::into`]'s validity, placement and stream
/// requirements. Retain all source and destination owners through checked completion.
/// After an error, retain them until [`crate::io::drain`] and every producer stream
/// succeed; an error does not prove quiescence. Output is unspecified after a runtime
/// or value error, while descriptor and overlap refusals precede writes.
#[cfg(feature = "cuda")]
#[allow(unsafe_code)]
pub unsafe fn decode_into(
    encoding: &crate::format::Encoding,
    dtype: &crate::format::Dtype,
    shape: &[u64],
    parts: &[DecodePart<'_>],
    out: DecodeOutput<'_>,
) -> Result<()> {
    // SAFETY: the caller supplies and retains all buffers under the contract above.
    unsafe { crate::io::decode::into(encoding, dtype, shape, parts, out) }
}

/// Fill caller-owned host or device destinations through the existing byte engine.
///
/// # Safety
/// Every source and destination must satisfy [`crate::io::read`]'s ownership and
/// pointer requirements. An error does not prove GPU completion: retain device
/// destinations and direct host/device source owners until [`crate::io::drain`]
/// succeeds for every participating device and any producer stream also drains.
#[allow(unsafe_code)]
pub unsafe fn read_into(
    checkpoint: &crate::Checkpoint,
    targets: &[crate::io::Target<'_>],
    options: &crate::io::ReadOpts,
) -> Result<crate::io::ReadStats> {
    // SAFETY: the caller supplies and retains every target under the contract above.
    unsafe { checkpoint.read_into_raw(targets, options) }
}

// For the Python binding only: runs `future` on the process-owned driver. Importing the
// binding neither calls it nor starts native workers.
#[doc(hidden)]
pub fn block_on<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    runtime()?.block_on(future)
}

pub(crate) fn runtime() -> Result<Arc<tokio::runtime::Runtime>> {
    crate::fetch::runtime().map_err(|error| {
        let detail = error.detail.clone();
        Failure {
            detail,
            ..Failure::from(error)
        }
    })
}
