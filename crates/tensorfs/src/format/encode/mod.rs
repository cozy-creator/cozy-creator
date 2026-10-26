//! The writer side of each scheme (spec §6): its complete `Encoding`, and a registry row with
//! its encoders. `quantize` is a closed form TensorFS runs itself (RTN, MX/NV FP, GGUF via
//! ggml); `pack` takes the codes, scales, zeros and permutation a calibrated producer computed
//! (GPTQ, AWQ, SVDQuant) and lays them out as the scheme's parts. Shared math: `elem` (f32 to
//! element codes) and `affine` (codes to packed words, the inverse of the decode's unpack).
//! `int8.convrot` requires `quantize_strategy` with the producer revision; it has no
//! anonymous default, since stored encoding semantics do not identify a quantizer.
//!
//! Adding a scheme's encoders: one family file, one row in `ENCODERS` (and its `mod` line).

use std::collections::BTreeMap;

use super::decode::PartBytes;
use super::{Dtype, Encoding, Error, Result, Value};

pub mod affine;
pub mod elem;

mod fp8;
mod int8;
mod mxfp8;
mod nvfp4;
mod svdq;
mod w4a8;

// fp8.blockwise, mxfp4
mod fp8_blockwise;
mod mxfp4;

// gguf
mod gguf;

// gptq, awq, ct
mod awq;
mod ct;
mod gptq;

// bnb
mod bnb;

// comfy
mod comfy;

pub use comfy::{Kitchen, quantize_with};
pub use fp8::FP8_ROWWISE_STRATEGY;
pub use fp8_blockwise::quantize_fp8_blockwise;
pub use int8::CONVROT_STRATEGY;
pub use nvfp4::MIN_SSE_STRATEGY;
pub use nvfp4::{GlobalScale, NibbleOrder};
pub use svdq::{
    SVDQ_FIT_STRATEGY, SVDQ_NUMERICAL_IDENTITY, SvdqFitted, svdq_fit, svdq_fit_workspace,
};
pub use w4a8::W4A8_STRATEGY;

/// Every scheme's encoders, one row each.
pub static ENCODERS: &[&Encoder] = &[
    &fp8::ROWWISE,
    &int8::CONVROT,
    &nvfp4::NVFP4,
    &svdq::SVDQ_NVFP4,
    // fp8.blockwise, mxfp4
    &fp8_blockwise::ENCODER,
    &mxfp4::ENCODER,
    // gguf.*
    &gguf::Q4_0,
    &gguf::Q4_1,
    &gguf::Q5_0,
    &gguf::Q5_1,
    &gguf::Q8_0,
    &gguf::Q2_K,
    &gguf::Q3_K,
    &gguf::Q4_K,
    &gguf::Q5_K,
    &gguf::Q6_K,
    &gguf::MXFP4,
    &gguf::NVFP4,
    &gguf::Q1_0,
    &gguf::Q2_0,
    // gptq.*, awq.*, ct.*
    &gptq::INT2,
    &gptq::INT3,
    &gptq::INT4,
    &gptq::INT8,
    &awq::INT4,
    &ct::INT1,
    &ct::INT2,
    &ct::INT3,
    &ct::INT4,
    &ct::INT5,
    &ct::INT6,
    &ct::INT7,
    &ct::INT8,
    // bnb.*
    &bnb::NF4,
    &bnb::FP4,
    &bnb::INT8,
    // comfy.*
    &comfy::ASYM_W4A8_INT8,
    &comfy::W4A8_CONVROT,
    &comfy::W6A8_INT8,
    &comfy::INT4_CONVROT,
];

/// An encoder: the parts of `input.encoding`, in the scheme's part order.
pub type Encode = fn(&Input) -> Result<Vec<OwnedPart>>;

pub struct Encoder {
    pub scheme: &'static str,
    /// From the logical values: `arrays["values"]`, `F32` in the logical shape.
    pub quantize: Option<Encode>,
    /// From a calibrated producer's arrays, by the producer's names.
    pub pack: Option<Encode>,
}

pub struct Input<'a> {
    /// The logical tensor's dtype and shape.
    pub dtype: &'a Dtype,
    pub shape: &'a [u64],
    /// The complete encoding to produce: every param its decode reads.
    pub encoding: &'a Encoding,
    pub arrays: &'a BTreeMap<&'a str, PartBytes<'a>>,
}

impl Input<'_> {
    /// `arrays["values"]`: the logical values, `F32` in the logical shape, all finite.
    pub fn values(&self) -> Result<Vec<f32>> {
        let fail = |d: &str| Error::Encode(format!("{}: {d}", self.encoding.scheme));
        let v = (self.arrays.get("values")).ok_or_else(|| fail("no values"))?;
        if *v.dtype != Dtype::F32 || v.shape != self.shape {
            return Err(fail("values are not F32 in the logical shape"));
        }
        let x = super::decode::elem::floats(v.dtype, v.bytes);
        if x.iter().any(|x| !x.is_finite()) {
            return Err(fail("a value is not finite"));
        }
        Ok(x)
    }
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

    pub fn merge(&mut self, other: &Self) {
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
pub fn implementation() -> String {
    let mut hash = crate::sha256::Sha256::new();
    for source in [
        include_str!("mod.rs"),
        include_str!("int8.rs"),
        include_str!("fp8.rs"),
        include_str!("w4a8.rs"),
        include_str!("comfy.rs"),
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

fn row(scheme: &str) -> Result<&'static Encoder> {
    ENCODERS
        .iter()
        .copied()
        .find(|e| e.scheme == scheme)
        .ok_or_else(|| Error::UnknownScheme(scheme.into()))
}

fn run(input: &Input, f: Option<Encode>, what: &str) -> Result<Vec<OwnedPart>> {
    let scheme = &input.encoding.scheme;
    let f = f.ok_or_else(|| Error::Encode(format!("{scheme}: no {what} encoder")))?;
    f(input)
}

/// The parts of `input.encoding` from the logical values (closed form).
pub fn quantize(input: &Input) -> Result<Vec<OwnedPart>> {
    if input.encoding.scheme == "int8.convrot" {
        return Err(Error::Encode(format!(
            "int8.convrot requires explicit quantizer strategy {CONVROT_STRATEGY}"
        )));
    }
    run(input, row(&input.encoding.scheme)?.quantize, "closed-form")
}

/// A producer with an explicit numerical revision, distinct from its stored encoding.
/// Other legacy encoders keep their existing API and cannot be selected accidentally.
pub fn quantize_strategy(input: &Input, strategy: &str) -> Result<Vec<OwnedPart>> {
    Ok(quantize_measured(input, strategy)?.parts)
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

/// The parts of `input.encoding` from a calibrated producer's arrays.
pub fn pack(input: &Input) -> Result<Vec<OwnedPart>> {
    run(input, row(&input.encoding.scheme)?.pack, "pack")
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

/// An integer array (`I8`, `U8`, `I16`, `U16` or `I32`) of a calibrated producer's, by name.
fn array(input: &Input, role: &str) -> Result<Vec<i32>> {
    let fail = |d: &str| Error::Encode(format!("{}: {role} {d}", input.encoding.scheme));
    let a = input.arrays.get(role).ok_or_else(|| fail("is missing"))?;
    let b = a.bytes;
    Ok(match a.dtype {
        Dtype::I8 => b.iter().map(|v| *v as i8 as i32).collect(),
        Dtype::U8 => b.iter().map(|v| *v as i32).collect(),
        Dtype::I16 => (b.as_chunks::<2>().0.iter())
            .map(|v| i16::from_le_bytes(*v) as i32)
            .collect(),
        Dtype::U16 => (b.as_chunks::<2>().0.iter())
            .map(|v| u16::from_le_bytes(*v) as i32)
            .collect(),
        Dtype::I32 => (b.as_chunks::<4>().0.iter())
            .map(|v| i32::from_le_bytes(*v))
            .collect(),
        other => return Err(fail(&format!("is {other}, not an integer array"))),
    })
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
