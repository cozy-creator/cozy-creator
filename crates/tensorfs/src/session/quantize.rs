//! Bounded tensor conversion through the existing checkpoint reader and Writer journal.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use super::{
    Checkpoint, Component, ConversionControl, Download, PartData, ReadInto, Selection, Writer,
};
use crate::failure::{Failure, Kind, Result, fail};
use crate::format::{Body, Dtype, Encoding, PartBytes, encode};

#[path = "quantize_fit.rs"]
mod fitting;
#[path = "quantize_bf16.rs"]
mod normalization;
pub use normalization::{Normalization, NormalizationResult, NormalizeBf16Tensor};

/// A fully resolved tensor, never a model selector or a Python callback.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuantizeTensor {
    pub component: String,
    pub key: String,
    pub source_dtype: Dtype,
    pub shape: Vec<u64>,
    pub dtype: Dtype,
    pub encoding: Encoding,
    pub strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<QuantizeFit>,
}

/// Exact immutable calibration tensor references for the versioned SVDQ fitter.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuantizeFit {
    pub seed: u64,
    pub component: String,
    pub xmax: String,
    pub rows: String,
}

pub struct Quantization<'a> {
    /// Full immutable selection, including entries already completed by this Writer.
    pub tensors: &'a [QuantizeTensor],
    pub calibration: Option<&'a Checkpoint>,
    /// Execute one component of the unchanged full selection. Not part of resume identity.
    pub component: Option<&'a str>,
    /// Conversion working buffers; the existing reader/Writer retain their own bounded I/O buffers.
    pub max_buffer_bytes: usize,
    /// Remaining encoded payload allowance, including every companion part, before deduplication.
    pub max_new_bytes: u64,
    pub max_relative_frobenius: Option<f64>,
    pub control: &'a ConversionControl,
}

#[derive(Debug, Serialize)]
pub struct QuantizedTensor {
    pub component: String,
    pub key: String,
    pub source_bytes_read: u64,
    pub new_bytes_written: u64,
    pub relative_frobenius: f64,
    pub measured: encode::QuantizationStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitDetails>,
}

#[derive(Debug, Serialize)]
pub struct FitDetails {
    pub strategy: String,
    pub alpha: f32,
    pub residual: encode::QuantizationStats,
    pub calibration_error: f64,
    pub plain_nvfp4_error: f64,
    pub calibration_bytes_read: u64,
    pub required_buffer_bytes: usize,
}

#[derive(Default, Debug, Serialize)]
pub struct QuantizationResult {
    pub identity: String,
    pub encoded_keys: usize,
    pub reused_keys: usize,
    pub source_bytes_read: u64,
    pub calibration_bytes_read: u64,
    pub required_buffer_bytes: usize,
    /// Logical newly encoded bytes, not physical store growth after deduplication.
    pub new_bytes_written: u64,
    pub saturated_elements: u64,
    pub worst_relative_frobenius: Option<f64>,
    pub tensors: Vec<QuantizedTensor>,
}

struct Pending<'a> {
    spec: &'a QuantizeTensor,
    component: Component,
    rows_per_chunk: usize,
    source: Source,
    row_source_bytes: u64,
    output_bytes: u64,
    parts: Vec<(String, Dtype, Vec<u64>)>,
    nvfp4: bool,
    error_fraction: f64,
    required_buffer_bytes: usize,
    fit: Option<fitting::PendingFit>,
}

enum Source {
    Plain {
        width: usize,
    },
    Rowwise {
        encoding: Encoding,
        weight: Dtype,
        scale: Dtype,
        scale_width: usize,
    },
}

fn source_layout(tensor: &crate::format::Tensor) -> Result<Source> {
    match &tensor.body {
        Body::Data(_) => Ok(Source::Plain {
            width: if tensor.dtype == Dtype::F32 { 4 } else { 2 },
        }),
        Body::Encoded { encoding, parts } => {
            if !matches!(encoding.scheme.as_str(), "fp8.rowwise" | "int8.rowwise")
                || !encoding.params.is_empty()
            {
                return fail(
                    Kind::Unsupported,
                    "quantization source needs plain floats or parameter-free fp8.rowwise/int8.rowwise",
                );
            }
            let metadata = parts
                .iter()
                .map(|part| {
                    Ok(crate::format::decode::PartMetadata {
                        role: &part.role,
                        dtype: &part.dtype,
                        shape: &part.shape,
                        len: usize::try_from(part.at().length)
                            .map_err(|_| invalid("source part exceeds address space"))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            crate::format::decode::check_metadata(&Dtype::F32, &tensor.shape, encoding, &metadata)?;
            let weight = parts
                .iter()
                .find(|part| part.role == "weight")
                .expect("validated weight");
            let scale = parts
                .iter()
                .find(|part| part.role == "scale")
                .expect("validated scale");
            Ok(Source::Rowwise {
                encoding: encoding.clone(),
                weight: weight.dtype.clone(),
                scale: scale.dtype.clone(),
                scale_width: if scale.dtype == Dtype::F32 { 4 } else { 2 },
            })
        }
    }
}

/// The same complete numerical/source-reader implementation identity used by Writer resume.
pub fn quantization_implementation() -> String {
    let mut hash = crate::sha256::Sha256::new();
    hash.update(encode::implementation().as_bytes());
    for source in [
        include_str!("quantize.rs"),
        include_str!("quantize_fit.rs"),
        include_str!("quantize_bf16.rs"),
        include_str!("writer.rs"),
        include_str!("../writer.rs"),
        include_str!("../format/decode/elem.rs"),
        include_str!("../format/decode/mod.rs"),
        include_str!("../format/decode/plan.rs"),
        include_str!("../format/decode/fp8.rs"),
        include_str!("../format/decode/int8.rs"),
        include_str!("../format/decode/affine.rs"),
        include_str!("../format/decode/grid.rs"),
    ] {
        hash.update(&(source.len() as u64).to_le_bytes());
        hash.update(source.as_bytes());
    }
    crate::sha256::hex(&hash.finish())
}

fn invalid(detail: impl Into<String>) -> Failure {
    Failure {
        kind: Kind::Invalid,
        detail: detail.into(),
    }
}
fn checked(value: Option<u64>) -> Result<u64> {
    value.ok_or_else(|| invalid("quantization shape or byte count overflows"))
}

impl Writer {
    /// Encode a full resolved selection. Entries publish one at a time after acceptance;
    /// this never commits the checkpoint. Cancellation is observed between bounded reads
    /// and writes, not a promise to interrupt a blocked filesystem syscall.
    pub async fn quantize(
        &mut self,
        source: &Checkpoint,
        options: &Quantization<'_>,
    ) -> Result<QuantizationResult> {
        if options.tensors.is_empty() || options.max_buffer_bytes == 0 {
            return fail(
                Kind::Invalid,
                "quantization needs a nonempty selection and explicit buffer bound",
            );
        }
        if options
            .max_relative_frobenius
            .is_some_and(|n| !n.is_finite() || n < 0.0)
        {
            return fail(
                Kind::Invalid,
                "relative Frobenius bound must be finite and nonnegative",
            );
        }
        options.control.check()?;
        if options.component.is_some_and(|component| {
            !options
                .tensors
                .iter()
                .any(|spec| spec.component == component)
        }) {
            return fail(
                Kind::Invalid,
                "quantization component filter is absent from the resolved selection",
            );
        }
        let mut names = BTreeSet::new();
        let mut pending = Vec::new();
        let mut output_bytes = 0u64;
        let mut input_bytes = 0u64;
        let mut reused = 0;
        for spec in options.tensors {
            let active = options
                .component
                .is_none_or(|component| spec.component == component);
            if !names.insert((spec.component.clone(), spec.key.clone())) {
                return fail(Kind::Invalid, "quantization selection repeats a tensor");
            }
            let component = source.component(&spec.component)?;
            let tensor = component
                .tensors()
                .iter()
                .find(|t| t.key == spec.key)
                .ok_or_else(|| {
                    invalid(format!(
                        "{}: no source tensor {:?}",
                        spec.component, spec.key
                    ))
                })?;
            if tensor.dtype != spec.source_dtype || tensor.shape != spec.shape {
                return fail(
                    Kind::Invalid,
                    "quantization source identity, dtype or shape differs from resolved selection",
                );
            }
            if !matches!(spec.source_dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
                return fail(
                    Kind::Unsupported,
                    "native quantization needs logical F16, BF16 or F32 source weights",
                );
            }
            let source = source_layout(tensor)?;
            if !matches!(spec.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
                return fail(
                    Kind::Invalid,
                    "quantization output logical dtype must be floating point",
                );
            }
            let [rows, cols] = spec.shape.as_slice() else {
                return fail(Kind::Invalid, "native quantization needs a rank-2 matrix");
            };
            if spec.strategy == encode::SVDQ_FIT_STRATEGY {
                let fit = fitting::preflight(spec, options, active)?;
                let row_source_bytes = match &source {
                    Source::Plain { width } => checked(cols.checked_mul(*width as u64))?,
                    Source::Rowwise { scale_width, .. } => {
                        checked(cols.checked_add(*scale_width as u64))?
                    }
                };
                let parts = fitting::parts(*rows, *cols);
                let bytes = parts.iter().try_fold(0u64, |sum, (_, dtype, shape)| {
                    checked(sum.checked_add(checked(dtype.byte_len(shape))?))
                })?;
                if active {
                    self.component_writer(&spec.component)?;
                }
                if active && self.has(&spec.component, &spec.key)? {
                    reused += 1;
                } else if active {
                    output_bytes = checked(output_bytes.checked_add(bytes))?;
                    pending.push(Pending {
                        spec,
                        component: component.clone(),
                        rows_per_chunk: 1,
                        source,
                        row_source_bytes,
                        output_bytes: bytes,
                        parts,
                        nvfp4: false,
                        error_fraction: 0.25,
                        required_buffer_bytes: fit.required_buffer_bytes,
                        fit: Some(fit),
                    });
                }
                continue;
            }
            if spec.fit.is_some() {
                return fail(
                    Kind::Invalid,
                    "fit references require the explicit SVDQ fitting strategy",
                );
            }
            let (nvfp4, block, error_fraction) = match (
                spec.encoding.scheme.as_str(),
                spec.strategy.as_str(),
            ) {
                ("int8.convrot", encode::CONVROT_STRATEGY)
                    if spec.encoding == Encoding::int8_convrot(256) =>
                {
                    (false, 256, 0.0625)
                }
                ("nvfp4", encode::MIN_SSE_STRATEGY)
                    if spec.encoding
                        == Encoding::nvfp4(
                            encode::GlobalScale::Multiply,
                            encode::NibbleOrder::LowFirst,
                            crate::format::ScaleLayout::RowMajor,
                        ) =>
                {
                    (true, 16, 0.25)
                }
                ("fp8.rowwise", encode::FP8_ROWWISE_STRATEGY)
                    if spec.encoding == Encoding::fp8_rowwise() =>
                {
                    (false, 1, 0.125)
                }
                ("w4a8.convrot", encode::W4A8_STRATEGY)
                    if spec.encoding == Encoding::w4a8_convrot(256) =>
                {
                    (false, 256, 0.5)
                }
                _ => {
                    return fail(
                        Kind::Unsupported,
                        "native quantization requires an exact supported encoding and numerical strategy",
                    );
                }
            };
            let w4a8 = spec.strategy == encode::W4A8_STRATEGY;
            let fp8 = spec.strategy == encode::FP8_ROWWISE_STRATEGY;
            if *rows == 0 || *cols == 0 || cols % block != 0 {
                return fail(
                    Kind::Invalid,
                    "quantization needs nonempty complete rows aligned to the encoding block",
                );
            }
            let source_bytes = checked(spec.source_dtype.byte_len(&spec.shape))?;
            usize::try_from(source_bytes)
                .map_err(|_| invalid("tensor shape exceeds this platform's address space"))?;
            // Source + F32 input + every encoded part coexist. ConvRot additionally owns
            // one F64 row; the fixed allowance covers block arithmetic/metadata scratch.
            let row_output = if nvfp4 || w4a8 {
                checked(
                    cols.checked_div(2)
                        .and_then(|n| n.checked_add(cols / 16))
                        .and_then(|n| n.checked_add(if w4a8 { 4 } else { 0 })),
                )?
            } else {
                checked(cols.checked_add(4))?
            };
            let (row_source_bytes, row_memory) = match &source {
                Source::Plain { width } => (
                    checked(cols.checked_mul(*width as u64))?,
                    checked(
                        cols.checked_mul(*width as u64 + 4)
                            .and_then(|n| n.checked_add(row_output)),
                    )?,
                ),
                Source::Rowwise { scale_width, .. } => {
                    let stored = checked(cols.checked_add(*scale_width as u64))?;
                    // The shared CPU plan holds I32 codes and F32 values together, then
                    // F32 values and serialized output together. Scales widen to F32.
                    let decoding = checked(
                        cols.checked_mul(9)
                            .and_then(|n| n.checked_add(*scale_width as u64 + 4)),
                    )?;
                    let encoding =
                        checked(cols.checked_mul(4).and_then(|n| n.checked_add(row_output)))?;
                    (stored, decoding.max(encoding))
                }
            };
            let source_bytes = checked(rows.checked_mul(row_source_bytes))?;
            let scratch = checked(
                (if nvfp4 || fp8 {
                    Some(0)
                } else {
                    cols.checked_mul(8)
                })
                .and_then(|n| n.checked_add(if w4a8 { cols / 2 } else { 0 }))
                .and_then(|n| n.checked_add(4096)),
            )?;
            let room = (options.max_buffer_bytes as u64).saturating_sub(scratch);
            let rows_per_chunk = (room / row_memory).min(*rows);
            if rows_per_chunk == 0 && active {
                return fail(
                    Kind::Full,
                    "one complete quantization row exceeds max_buffer_bytes",
                );
            }
            let bytes = checked(rows.checked_mul(row_output).and_then(|n| {
                n.checked_add(if nvfp4 {
                    4
                } else if w4a8 {
                    64
                } else {
                    0
                })
            }))?;
            let parts = if nvfp4 {
                vec![
                    ("weight".into(), Dtype::F4, spec.shape.clone()),
                    ("scale".into(), Dtype::F8_E4M3, vec![*rows, *cols / 16]),
                    ("scale_2".into(), Dtype::F32, vec![]),
                ]
            } else if w4a8 {
                vec![
                    ("weight".into(), Dtype::I8, vec![*rows, *cols / 2]),
                    (
                        "group_scale".into(),
                        Dtype::F8_E4M3,
                        vec![*rows, *cols / 16],
                    ),
                    ("scale".into(), Dtype::F32, vec![*rows]),
                    ("codebook".into(), Dtype::F32, vec![16]),
                ]
            } else {
                vec![
                    (
                        "weight".into(),
                        if fp8 { Dtype::F8_E4M3 } else { Dtype::I8 },
                        spec.shape.clone(),
                    ),
                    ("scale".into(), Dtype::F32, vec![*rows]),
                ]
            };
            if active {
                self.component_writer(&spec.component)?; // Validate every executed destination before mutation.
            }
            if active && self.has(&spec.component, &spec.key)? {
                reused += 1;
            } else if active {
                output_bytes = checked(output_bytes.checked_add(bytes))?;
                input_bytes = checked(input_bytes.checked_add(checked(
                    source_bytes.checked_mul(if nvfp4 { 2 } else { 1 }),
                )?))?;
                pending.push(Pending {
                    spec,
                    component: component.clone(),
                    rows_per_chunk: rows_per_chunk as usize,
                    source,
                    row_source_bytes,
                    output_bytes: bytes,
                    parts,
                    nvfp4,
                    error_fraction,
                    required_buffer_bytes: (scratch + rows_per_chunk * row_memory) as usize,
                    fit: None,
                });
            }
        }
        if output_bytes > options.max_new_bytes {
            return fail(
                Kind::Full,
                format!(
                    "quantization needs {output_bytes} encoded bytes, allowance is {}",
                    options.max_new_bytes
                ),
            );
        }
        let identity = crate::sha256::hex_digest(
            &serde_json::to_vec(&(
                "tensorfs-quantization/1",
                quantization_implementation(),
                source.id(),
                options.calibration.map(Checkpoint::id),
                self.base_id(),
                options.tensors,
                options.max_relative_frobenius,
            ))
            .map_err(|e| invalid(e.to_string()))?,
        );
        self.bind_quantization(&identity, &names.into_iter().collect::<Vec<_>>())?;
        let mut result = QuantizationResult {
            identity,
            reused_keys: reused,
            worst_relative_frobenius: (reused == 0).then_some(0.0),
            ..Default::default()
        };
        for pending in pending {
            options.control.check()?;
            let row = if pending.fit.is_some() {
                self.quantize_fit(source, &pending, options).await?
            } else {
                self.quantize_one(source, &pending, options).await?
            };
            result.encoded_keys += 1;
            result.source_bytes_read += row.source_bytes_read;
            result.new_bytes_written += row.new_bytes_written;
            result.required_buffer_bytes = result
                .required_buffer_bytes
                .max(pending.required_buffer_bytes);
            result.calibration_bytes_read +=
                row.fit.as_ref().map_or(0, |fit| fit.calibration_bytes_read);
            result.saturated_elements += row.measured.saturated;
            if let Some(worst) = &mut result.worst_relative_frobenius {
                *worst = worst.max(row.relative_frobenius);
            }
            result.tensors.push(row);
        }
        Ok(result)
    }

    async fn quantize_one(
        &mut self,
        source: &Checkpoint,
        pending: &Pending<'_>,
        options: &Quantization<'_>,
    ) -> Result<QuantizedTensor> {
        let spec = pending.spec;
        let rows = spec.shape[0] as usize;
        let cols = spec.shape[1] as usize;
        let mut read_bytes = 0u64;
        let mut maximum = 0.0f64;
        if pending.nvfp4 {
            for start in (0..rows).step_by(pending.rows_per_chunk) {
                let count = pending.rows_per_chunk.min(rows - start);
                let values = read_window(source, pending, start, count, options.control).await?;
                read_bytes += count as u64 * pending.row_source_bytes;
                for raw in values.as_chunks::<4>().0 {
                    let value = f32::from_le_bytes(*raw);
                    if !value.is_finite() {
                        return fail(
                            Kind::Decode,
                            "quantization source contains nonfinite values",
                        );
                    }
                    maximum = maximum.max(f64::from(value.abs()));
                }
            }
        }
        let mut staged = pending
            .parts
            .iter()
            .map(|_| self.scratch())
            .collect::<Result<Vec<_>>>()?;
        let mut measured = encode::QuantizationStats::default();
        for start in (0..rows).step_by(pending.rows_per_chunk) {
            let count = pending.rows_per_chunk.min(rows - start);
            let values = read_window(source, pending, start, count, options.control).await?;
            read_bytes += count as u64 * pending.row_source_bytes;
            let shape = [count as u64, cols as u64];
            let amax = maximum.to_le_bytes();
            let mut arrays = BTreeMap::from([(
                "values",
                PartBytes {
                    dtype: &Dtype::F32,
                    shape: &shape,
                    bytes: &values,
                },
            )]);
            if pending.nvfp4 {
                arrays.insert(
                    "tensor_amax",
                    PartBytes {
                        dtype: &Dtype::F64,
                        shape: &[],
                        bytes: &amax,
                    },
                );
            }
            let encoded = encode::quantize_measured(
                &encode::Input {
                    dtype: &spec.dtype,
                    shape: &shape,
                    encoding: &spec.encoding,
                    arrays: &arrays,
                },
                &spec.strategy,
            )?;
            if encoded.parts.len() != pending.parts.len() {
                return fail(Kind::Integrity, "quantizer changed its resolved part count");
            }
            measured.merge(&encoded.stats);
            for (index, part) in encoded.parts.iter().enumerate() {
                let (role, dtype, geometry) = &pending.parts[index];
                let mut expected = geometry.clone();
                let shared = geometry.is_empty() || role == "codebook";
                if let Some(rows) = expected.first_mut().filter(|_| !shared) {
                    *rows = count as u64;
                }
                if part.role != *role || part.dtype != *dtype || part.shape != expected {
                    return fail(
                        Kind::Integrity,
                        "quantizer changed its resolved part geometry",
                    );
                }
                if !shared || start == 0 {
                    options.control.check()?;
                    staged[index]
                        .file()
                        .write_all(&part.bytes)
                        .map_err(|e| super::io_failure("quantization staging", e))?;
                }
            }
        }
        let relative = if measured.energy > 0.0 {
            (measured.sse / measured.energy).sqrt()
        } else {
            0.0
        };
        let bound = measured.amax * pending.error_fraction;
        if !measured.max_abs_error.is_finite()
            || measured.max_abs_error > bound
            || !relative.is_finite()
            || options
                .max_relative_frobenius
                .is_some_and(|limit| relative > limit)
        {
            return fail(
                Kind::Decode,
                format!(
                    "quantization_tripwire: {}.{} max error {}, bound {bound}, relative Frobenius {relative}",
                    spec.component, spec.key, measured.max_abs_error
                ),
            );
        }
        for (file, (_, dtype, shape)) in staged.iter_mut().zip(&pending.parts) {
            if file
                .file()
                .metadata()
                .map_err(|e| super::io_failure("quantization staging", e))?
                .len()
                != dtype
                    .byte_len(shape)
                    .ok_or_else(|| invalid("encoded part size overflows"))?
            {
                return fail(Kind::Integrity, "quantizer emitted a different part length");
            }
            file.file()
                .seek(SeekFrom::Start(0))
                .map_err(|e| super::io_failure("quantization staging", e))?;
        }
        options.control.check()?;
        let mut readers: Vec<_> = staged
            .iter_mut()
            .map(|part| CheckedRead {
                source: part.file(),
                control: options.control,
            })
            .collect();
        let parts = readers
            .iter_mut()
            .zip(&pending.parts)
            .map(|(reader, (role, dtype, shape))| PartData {
                role: role.clone(),
                dtype: dtype.clone(),
                shape: shape.clone(),
                data: reader,
            })
            .collect();
        let written = self.component_writer(&spec.component)?.put_encoded(
            &spec.key,
            spec.dtype.clone(),
            &spec.shape,
            spec.encoding.clone(),
            parts,
        );
        if written.is_err() {
            options.control.check()?;
        }
        written?;
        options
            .control
            .written_tensors
            .fetch_add(1, Ordering::Release);
        Ok(QuantizedTensor {
            component: spec.component.clone(),
            key: spec.key.clone(),
            source_bytes_read: read_bytes,
            new_bytes_written: pending.output_bytes,
            relative_frobenius: relative,
            measured,
            fit: None,
        })
    }
}

async fn read_window(
    source: &Checkpoint,
    pending: &Pending<'_>,
    start: usize,
    count: usize,
    control: &ConversionControl,
) -> Result<Vec<u8>> {
    FloatReader {
        checkpoint: source,
        component: &pending.component,
        component_name: &pending.spec.component,
        key: &pending.spec.key,
        dtype: &pending.spec.source_dtype,
        layout: &pending.source,
        columns: pending.spec.shape[1] as usize,
    }
    .read(start, count, control)
    .await
}

/// Shared bounded F32 source windows for normalization and encoded quantization.
struct FloatReader<'a> {
    checkpoint: &'a Checkpoint,
    component: &'a Component,
    component_name: &'a str,
    key: &'a str,
    dtype: &'a Dtype,
    layout: &'a Source,
    columns: usize,
}

impl FloatReader<'_> {
    async fn read(
        &self,
        start: usize,
        count: usize,
        control: &ConversionControl,
    ) -> Result<Vec<u8>> {
        control.check()?;
        let cols = self.columns;
        match self.layout {
            Source::Plain { width } => {
                let bytes = read_named_part(
                    self.checkpoint,
                    self.component,
                    self.component_name,
                    self.key,
                    None,
                    (start * cols * width) as u64,
                    count * cols * width,
                    control,
                )
                .await?;
                if *self.dtype == Dtype::F32 {
                    return Ok(bytes);
                }
                let mut values = Vec::new();
                values
                    .try_reserve_exact(count * cols * 4)
                    .map_err(|_| Failure {
                        kind: Kind::Full,
                        detail: "quantization decode allocation failed".into(),
                    })?;
                for raw in bytes.as_chunks::<2>().0 {
                    let bits = u16::from_le_bytes(*raw);
                    let value = if *self.dtype == Dtype::BF16 {
                        crate::format::decode::elem::bf16(bits)
                    } else {
                        crate::format::decode::elem::f16(bits)
                    };
                    values.extend_from_slice(&value.to_le_bytes());
                }
                Ok(values)
            }
            Source::Rowwise {
                encoding,
                weight,
                scale,
                scale_width,
            } => {
                let codes = read_named_part(
                    self.checkpoint,
                    self.component,
                    self.component_name,
                    self.key,
                    Some("weight"),
                    (start * cols) as u64,
                    count * cols,
                    control,
                )
                .await?;
                let scales = read_named_part(
                    self.checkpoint,
                    self.component,
                    self.component_name,
                    self.key,
                    Some("scale"),
                    (start * scale_width) as u64,
                    count * scale_width,
                    control,
                )
                .await?;
                let shape = [count as u64, cols as u64];
                let scale_shape = [count as u64];
                let parts = BTreeMap::from([
                    (
                        "weight",
                        PartBytes {
                            dtype: weight,
                            shape: &shape,
                            bytes: &codes,
                        },
                    ),
                    (
                        "scale",
                        PartBytes {
                            dtype: scale,
                            shape: &scale_shape,
                            bytes: &scales,
                        },
                    ),
                ]);
                // The source's logical BF16/F16 dtype is metadata, not an extra rounding
                // boundary: Runtime's model-values contract reconstructs directly into F32.
                Ok(crate::format::decode::decode(
                    &Dtype::F32,
                    &shape,
                    encoding,
                    &parts,
                )?)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn read_named_part(
    source: &Checkpoint,
    component: &Component,
    component_name: &str,
    key: &str,
    role: Option<&str>,
    offset: u64,
    length: usize,
    control: &ConversionControl,
) -> Result<Vec<u8>> {
    control.check()?;
    let wanted = Download {
        selection: Selection::Part {
            component: component_name.to_owned(),
            key: key.to_owned(),
            role: role.map(str::to_owned),
            range: Some(offset..offset + length as u64),
        },
        ..Default::default()
    };
    tokio::select! {
        fetched = source.download(&wanted) => { fetched?; }
        () = control.cancelled() => { control.check()?; }
    }
    control.check()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| Failure {
        kind: Kind::Full,
        detail: "quantization read allocation failed".into(),
    })?;
    bytes.resize(length, 0u8);
    component.read_into(
        key,
        &mut bytes,
        &ReadInto {
            offset,
            role,
            ..Default::default()
        },
    )?;
    control
        .read_bytes
        .fetch_add(length as u64, Ordering::Release);
    control.check()?;
    Ok(bytes)
}

struct CheckedRead<'a> {
    source: &'a mut std::fs::File,
    control: &'a ConversionControl,
}
impl Read for CheckedRead<'_> {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        self.control.check().map_err(std::io::Error::other)?;
        let read = self.source.read(into)?;
        self.control
            .written_bytes
            .fetch_add(read as u64, Ordering::Release);
        Ok(read)
    }
}
