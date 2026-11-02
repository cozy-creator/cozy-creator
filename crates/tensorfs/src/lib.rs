//! TensorFS: the cozytensors format, its store, fetch and convert, and the read engine.

// Unsafe code lives only in platform, the OS seam.
#![deny(unsafe_code)]

#[cfg(feature = "store")]
pub(crate) mod descriptors;
pub mod err;
#[cfg(feature = "store")]
pub mod export;
pub mod failure;
pub use failure::{Failure, Kind, Result};
/// Model families: each first-party model's canonical form, verified at ingest.
pub mod family;
#[cfg(feature = "store")]
pub mod fetch;
pub mod format;
pub mod home;
#[cfg(feature = "store")]
pub mod import;
/// The read engine: byte ranges of files into host or device destinations (xs-055).
pub mod io;
pub mod kernels;
/// A header's precision family, by the lane rule's table (spec §7).
pub mod precision;
pub mod sha256;
#[cfg(feature = "store")]
pub mod store;
#[cfg(feature = "store")]
pub mod writer;
#[cfg(feature = "store")]
mod verification;
#[cfg(feature = "store")]
pub use verification::VerificationReport;
/// Advanced engine and synchronous-language integration.
#[cfg(feature = "store")]
pub mod raw;

/// The public session API; networking uses the caller's async runtime.
#[cfg(feature = "store")]
mod session;
#[cfg(feature = "store")]
pub use session::{
    bearer, Checkpoint, CheckpointFile, Client, Component, ComponentWriter, Config, Convert,
    ConversionControl, ConvertProgress, Download, Endpoint, Fetched, Format, HeaderMap, Open, Progress,
    FitDetails, Normalization, NormalizationResult, NormalizeBf16Tensor, Quantization, QuantizationResult, QuantizedTensor, QuantizeFit, QuantizeTensor, Read, quantization_implementation,
    ReadInto, Ref, Request, Selection, TokenProvider, Writer, WriterOptions,
};

/// This TensorFS's release version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The on-disk store this build reads and writes, shared by every process on a machine. A
/// change an older build would misread bumps it (`tensorfs.store/3`: trees and D10 headers).
pub const STORE_FORMAT: u32 = 3;
