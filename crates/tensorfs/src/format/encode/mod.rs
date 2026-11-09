//! The writer side of each scheme TensorFS produces (spec §6): its complete `Encoding`, and the
//! versioned producers `quantize_measured` runs (fp8.rowwise, int8.convrot, nvfp4's minimum-error
//! search, w4a8.convrot, SVDQuant's fit). Shared math: `elem` (f32 to element codes) and `affine`
//! (codes to packed words, the inverse of the decode's unpack). A producer has an explicit
//! numerical revision: stored encoding semantics do not identify a quantizer.

use std::collections::BTreeMap;

use super::decode::PartBytes;
use super::{Dtype, Encoding, Error, Result, Value};

pub(crate) mod affine;
pub(crate) mod elem;

mod fp8;
mod int8;
mod mxfp8;
mod nvfp4;
mod svdq;
mod w4a8;

pub(crate) use fp8::FP8_ROWWISE_STRATEGY;
pub(crate) use int8::CONVROT_STRATEGY;
pub(crate) use nvfp4::MIN_SSE_STRATEGY;
pub use nvfp4::{GlobalScale, NibbleOrder};
#[cfg(feature = "quantize")]
pub(crate) use svdq::{
    SVDQ_FIT_STRATEGY, svdq_fit, svdq_fit_workspace,
};
pub(crate) use w4a8::W4A8_STRATEGY;

pub struct Input<'a> {
    /// The logical tensor's dtype and shape.
    pub dtype: &'a Dtype,
    pub shape: &'a [u64],
    /// The complete encoding to produce: every param its decode reads.
    pub encoding: &'a Encoding,
    pub arrays: &'a BTreeMap<&'a str, PartBytes<'a>>,
}

impl Input<'_> {
}

/// One encoded part, in the scheme's part order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPart {
    pub role: String,
    pub dtype: Dtype,
    pub shape: Vec<u64>,
    pub bytes: Vec<u8>,
}

/// Producer acceptance facts, accumulated in F64 over the original F32 values.
/// Payload bytes are invariant to chunking; sums may differ by floating reduction order.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct QuantizationStats {
    pub amax: f64,
    pub max_abs_error: f64,
    pub sse: f64,
    pub energy: f64,
    pub saturated: u64,
    #[serde(skip)]
    sse_correction: f64,
    #[serde(skip)]
    energy_correction: f64,
}

impl QuantizationStats {
    fn accumulate(sum: &mut f64, correction: &mut f64, value: f64) {
        let adjusted = value - *correction;
        let next = *sum + adjusted;
        *correction = (next - *sum) - adjusted;
        *sum = next;
    }

    pub(crate) fn add(&mut self, source: f32, reconstructed: f64) {
        let source = f64::from(source);
        let error = reconstructed - source;
        self.amax = self.amax.max(source.abs());
        self.max_abs_error = self.max_abs_error.max(error.abs());
        Self::accumulate(&mut self.sse, &mut self.sse_correction, error * error);
        Self::accumulate(
            &mut self.energy,
            &mut self.energy_correction,
            source * source,
        );
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.amax = self.amax.max(other.amax);
        self.max_abs_error = self.max_abs_error.max(other.max_abs_error);
        Self::accumulate(&mut self.sse, &mut self.sse_correction, other.sse);
        Self::accumulate(&mut self.energy, &mut self.energy_correction, other.energy);
        self.saturated += other.saturated;
    }
}

pub struct Quantized {
    pub parts: Vec<OwnedPart>,
    pub stats: QuantizationStats,
}

/// A development build must not resume output from different numerical code under
/// an unchanged package version. Production memoization still names its immutable wheel.
pub(crate) fn implementation() -> String {
    let mut hash = crate::sha256::Sha256::new();
    for source in [
        include_str!("mod.rs"),
        include_str!("int8.rs"),
        include_str!("fp8.rs"),
        include_str!("w4a8.rs"),
        include_str!("affine.rs"),
        include_str!("nvfp4.rs"),
        include_str!("svdq.rs"),
        include_str!("svdq_fit.rs"),
        include_str!("../../../Cargo.toml"),
        include_str!("../../../../../Cargo.lock"),
        include_str!("../../../../../vendor/faer/TENSORFS-PATCH.json"),
        include_str!("../../../../../vendor/faer/src/linalg/svd/mod.rs"),
        include_str!("elem.rs"),
        include_str!("../decode/elem.rs"),
    ] {
        hash.update(&(source.len() as u64).to_le_bytes());
        hash.update(source.as_bytes());
    }
    crate::sha256::hex(&hash.finish())
}

/// The same versioned producer, including acceptance facts before publication.
pub fn quantize_measured(input: &Input, strategy: &str) -> Result<Quantized> {
    match (input.encoding.scheme.as_str(), strategy) {
        ("fp8.rowwise", FP8_ROWWISE_STRATEGY) => fp8::rowwise_measured(input),
        ("w4a8.convrot", W4A8_STRATEGY) => w4a8::fixed_measured(input),
        ("int8.convrot", CONVROT_STRATEGY) => int8::convrot_measured(input),
        ("nvfp4", MIN_SSE_STRATEGY) => nvfp4::min_sse(input),
        _ => Err(Error::Encode(format!(
            "no quantizer strategy {strategy:?} for {}",
            input.encoding.scheme
        ))),
    }
}

/// Exact F32 row bytes for a versioned, row-independent producer. No decode allocation.
fn value_rows<'a>(input: &Input<'a>, block: usize) -> Result<(usize, usize, &'a [u8])> {
    let fail = |detail: &str| Error::Encode(format!("{}: {detail}", input.encoding.scheme));
    if !matches!(input.dtype, Dtype::F32 | Dtype::F16 | Dtype::BF16) {
        return Err(fail("logical dtype must be F32, F16 or BF16"));
    }
    let [rows, cols] = input.shape else {
        return Err(fail("values must be a nonempty rank-2 matrix"));
    };
    if *rows == 0 || *cols == 0 || !cols.is_multiple_of(block as u64) {
        return Err(fail(
            "values need nonempty rows aligned to the producer block",
        ));
    }
    let rows = usize::try_from(*rows).map_err(|_| fail("row count overflows"))?;
    let cols = usize::try_from(*cols).map_err(|_| fail("column count overflows"))?;
    // This also proves rows*cols, rows*4 and cols*4 fit the address space below.
    let bytes = rows
        .checked_mul(cols)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| fail("F32 byte count overflows"))?;
    let source = input
        .arrays
        .get("values")
        .ok_or_else(|| fail("no values"))?;
    if input.arrays.len() != 1 {
        return Err(fail(
            "this strategy accepts only values, no shared tensor_amax",
        ));
    }
    if *source.dtype != Dtype::F32 || source.shape != input.shape || source.bytes.len() != bytes {
        return Err(fail(
            "values must be exactly F32 bytes in the logical shape",
        ));
    }
    Ok((rows, cols, source.bytes))
}

/// The Writer includes these buffers in its preflight allowance; allocation failure is typed.
fn reserve<T>(length: usize, label: &str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| Error::Encode(format!("cannot allocate {label}")))?;
    Ok(values)
}

/// An `Encoding` from its scheme and params.
pub(crate) fn encoding(scheme: &str, params: &[(&str, Value)]) -> Encoding {
    Encoding {
        scheme: scheme.into(),
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

fn part(
    role: &str,
    dtype: Dtype,
    shape: impl IntoIterator<Item = usize>,
    bytes: Vec<u8>,
) -> OwnedPart {
    OwnedPart {
        role: role.into(),
        dtype,
        shape: shape.into_iter().map(|e| e as u64).collect(),
        bytes,
    }
}
