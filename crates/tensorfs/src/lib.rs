//! TensorFS: the cozytensors format, its store, fetch and convert, and the read engine.
#![deny(unreachable_pub)]

// Unsafe code lives only in platform, the OS seam.
#![deny(unsafe_code)]

pub use tensorfs_core::format;
pub use tensorfs_core::store;
pub use tensorfs_core::io;
pub use tensorfs_core::writer;
pub use tensorfs_core::err;
pub use tensorfs_core::{Failure, Kind, Result};
pub use tensorfs_core::VerificationReport;
pub use tensorfs_core::STORE_FORMAT;
use tensorfs_core::{descriptors, failure, home, sha256, verification};

pub mod export;
/// Model families: each first-party model's canonical form, verified at ingest.
pub mod family;
pub mod fetch;
pub mod import;
/// A header's precision family, by the lane rule's table (spec §7).
pub mod precision;
/// Advanced engine and synchronous-language integration.
pub mod raw;

/// The public session API; networking uses the caller's async runtime.
mod session;
pub use session::{
    bearer, Checkpoint, CheckpointFile, Client, Component, ComponentWriter, Config, Convert,
    ConversionControl, ConvertProgress, Download, Endpoint, Fetched, Format, HeaderMap, Open, Progress,
    Read, ReadInto, Ref, Request, Selection, TokenProvider, Writer, WriterOptions,
};
#[cfg(feature = "quantize")]
pub use session::{
    FitDetails, Normalization, NormalizationResult, NormalizeBf16Tensor, Quantization, QuantizationResult,
    QuantizedTensor, QuantizeFit, QuantizeTensor, quantization_implementation,
};

/// A writer of `store` that re-verifies each family claim before it commits (every writer tensorfs makes).
pub(crate) fn writer(store: &store::Store, base: Option<&store::Checkpoint>, key: Option<&str>) -> store::Result<writer::Writer> {
    let mut writer = store.writer(base, key)?;
    writer.before_commit(family::reverify);
    Ok(writer)
}
