//! TensorFS: the cozytensors format, its store, writer and read engine; with `net` (on by default), fetch, import,
//! model families and the Client API. Without `net` it has no network or async runtime.
#![deny(unreachable_pub)]

// Unsafe code is denied by default and allowed only at the explicit native boundary modules.
#![deny(unsafe_code)]

/// File descriptor headroom: how many a process may hold open, and what's left.
pub(crate) mod descriptors;
pub mod err;
pub mod failure;
pub use failure::{Failure, Kind, Result};
pub mod format;
pub mod home;
/// The read engine: byte ranges of files into host or device destinations (xs-055).
pub mod io;
pub mod kernels;
/// A header's precision family, by the lane rule's table (spec §7).
pub mod precision;
pub mod sha256;
pub mod store;
pub mod writer;
/// A checkpoint's objects held to their digests.
#[cfg(feature = "net")]
mod verification;
#[cfg(feature = "net")]
pub use verification::VerificationReport;

#[cfg(feature = "net")]
pub mod export;
/// Model families: each first-party model's canonical form, verified at ingest.
#[cfg(feature = "net")]
pub mod family;
#[cfg(feature = "net")]
pub mod fetch;
#[cfg(feature = "net")]
pub mod import;
/// Advanced engine and synchronous-language integration.
#[cfg(feature = "net")]
pub mod raw;

/// The public session API; networking uses the caller's async runtime.
#[cfg(feature = "net")]
mod session;
#[cfg(feature = "net")]
pub use session::{
    bearer, Boxed, Checkpoint, CheckpointFile, Client, Component, ComponentWriter, Config, Convert,
    ConversionControl, ConvertProgress, Download, Endpoint, Fetched, Format, HeaderMap, Open, Progress,
    Read, ReadInto, Ref, Request, Selection, TokenProvider, Writer, WriterOptions,
};
#[cfg(all(feature = "net", feature = "quantize"))]
pub use session::{
    FitDetails, Normalization, NormalizationResult, NormalizeBf16Tensor, Quantization, QuantizationResult,
    QuantizedTensor, QuantizeFit, QuantizeTensor, quantization_implementation,
};

/// A writer of `store` that re-verifies each family claim before it commits (every writer tensorfs makes).
#[cfg(feature = "net")]
pub(crate) fn writer(store: &store::Store, base: Option<&store::Checkpoint>, key: Option<&str>) -> store::Result<writer::Writer> {
    let mut writer = store.writer(base, key)?;
    writer.before_commit(family::reverify);
    Ok(writer)
}

/// The on-disk store this build reads and writes, shared by every process on a machine. A
/// change an older build would misread bumps it (`tensorfs.store/3`: trees and D10 headers).
pub const STORE_FORMAT: u32 = 3;

/// This TensorFS's release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
