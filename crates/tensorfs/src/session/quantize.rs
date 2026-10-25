//! Bounded tensor conversion through the existing checkpoint reader and Writer journal.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use super::{
    Checkpoint, Component, ConversionControl, Download, PartData, ReadInto, Selection, Writer,
};
use crate::failure::{fail, Failure, Kind, Result};
use crate::format::{encode, Body, Dtype, Encoding, PartBytes};

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
}

pub struct Quantization<'a> {
    /// Full immutable selection, including entries already completed by this Writer.
    pub tensors: &'a [QuantizeTensor],
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
}

#[derive(Default, Debug, Serialize)]
pub struct QuantizationResult {
    pub identity: String,
    pub encoded_keys: usize,
    pub reused_keys: usize,
    pub source_bytes_read: u64,
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
    source_width: usize,
    output_bytes: u64,
    parts: Vec<(String, Dtype, Vec<u64>)>,
    nvfp4: bool,
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
        let mut names = BTreeSet::new();
        let mut pending = Vec::new();
        let mut output_bytes = 0u64;
        let mut input_bytes = 0u64;
        let mut reused = 0;
        for spec in options.tensors {
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
            if !matches!(tensor.body, Body::Data(_))
                || tensor.dtype != spec.source_dtype
                || tensor.shape != spec.shape
            {
                return fail(
                    Kind::Invalid,
                    "quantization source identity, dtype or shape differs from resolved selection",
                );
            }
            let width = match spec.source_dtype {
                Dtype::F16 | Dtype::BF16 => 2usize,
                Dtype::F32 => 4,
                _ => {
                    return fail(
                        Kind::Unsupported,
                        "native quantization reads plain F16, BF16 or F32 weights",
                    )
                }
            };
            if !matches!(spec.dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32) {
                return fail(
                    Kind::Invalid,
                    "quantization output logical dtype must be floating point",
                );
            }
            let [rows, cols] = spec.shape.as_slice() else {
                return fail(Kind::Invalid, "native quantization needs a rank-2 matrix");
            };
            let nvfp4 = match (spec.encoding.scheme.as_str(), spec.strategy.as_str()) {
                ("int8.convrot", encode::CONVROT_STRATEGY) if spec.encoding == Encoding::int8_convrot(256) => false,
                ("nvfp4", encode::MIN_SSE_STRATEGY) if spec.encoding == Encoding::nvfp4(
                    encode::GlobalScale::Multiply, encode::NibbleOrder::LowFirst, crate::format::ScaleLayout::RowMajor,
                ) => true,
                _ => return fail(Kind::Unsupported, "native quantization requires an exact supported encoding and numerical strategy"),
            };
            let block = if nvfp4 { 16 } else { 256 };
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
            let row_output = if nvfp4 {
                checked(cols.checked_div(2).and_then(|n| n.checked_add(cols / 16)))?
            } else {
                checked(cols.checked_add(4))?
            };
            let row_memory = checked(
                cols.checked_mul(width as u64 + 4)
                    .and_then(|n| n.checked_add(row_output)),
            )?;
            let scratch = checked(
                (if nvfp4 { Some(0) } else { cols.checked_mul(8) })
                    .and_then(|n| n.checked_add(4096)),
            )?;
            let room = (options.max_buffer_bytes as u64).saturating_sub(scratch);
            let rows_per_chunk = (room / row_memory).min(*rows);
            if rows_per_chunk == 0 {
                return fail(
                    Kind::Full,
                    "one complete quantization row exceeds max_buffer_bytes",
                );
            }
            let bytes = checked(
                rows.checked_mul(row_output)
                    .and_then(|n| n.checked_add(if nvfp4 { 4 } else { 0 })),
            )?;
            let parts = if nvfp4 {
                vec![
                    ("weight".into(), Dtype::F4, spec.shape.clone()),
                    ("scale".into(), Dtype::F8_E4M3, vec![*rows, *cols / 16]),
                    ("scale_2".into(), Dtype::F32, vec![]),
                ]
            } else {
                vec![
                    ("weight".into(), Dtype::I8, spec.shape.clone()),
                    ("scale".into(), Dtype::F32, vec![*rows]),
                ]
            };
            self.component_writer(&spec.component)?; // Validate every destination before mutation.
            if self.has(&spec.component, &spec.key)? {
                reused += 1;
            } else {
                output_bytes = checked(output_bytes.checked_add(bytes))?;
                input_bytes = checked(input_bytes.checked_add(checked(
                    source_bytes.checked_mul(if nvfp4 { 2 } else { 1 }),
                )?))?;
                pending.push(Pending {
                    spec,
                    component: component.clone(),
                    rows_per_chunk: rows_per_chunk as usize,
                    source_width: width,
                    output_bytes: bytes,
                    parts,
                    nvfp4,
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
                encode::implementation(),
                source.id(),
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
            let row = self.quantize_one(source, &pending, options).await?;
            result.encoded_keys += 1;
            result.source_bytes_read += row.source_bytes_read;
            result.new_bytes_written += row.new_bytes_written;
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
                read_bytes += (count * cols * pending.source_width) as u64;
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
            read_bytes += (count * cols * pending.source_width) as u64;
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
                if let Some(rows) = expected.first_mut() {
                    *rows = count as u64;
                }
                if part.role != *role || part.dtype != *dtype || part.shape != expected {
                    return fail(
                        Kind::Integrity,
                        "quantizer changed its resolved part geometry",
                    );
                }
                if !geometry.is_empty() || start == 0 {
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
        let bound = measured.amax * if pending.nvfp4 { 0.25 } else { 0.0625 };
        if !measured.max_abs_error.is_finite()
            || measured.max_abs_error > bound
            || !relative.is_finite()
            || options
                .max_relative_frobenius
                .is_some_and(|limit| relative > limit)
        {
            return fail(Kind::Decode, format!("quantization_tripwire: {}.{} max error {}, bound {bound}, relative Frobenius {relative}", spec.component, spec.key, measured.max_abs_error));
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
    control.check()?;
    let cols = pending.spec.shape[1] as usize;
    let offset = (start * cols * pending.source_width) as u64;
    let length = count * cols * pending.source_width;
    let wanted = Download {
        selection: Selection::Part {
            component: pending.spec.component.clone(),
            key: pending.spec.key.clone(),
            role: None,
            range: Some(offset..offset + length as u64),
        },
        ..Default::default()
    };
    tokio::select! {
        fetched = source.download(&wanted) => { fetched?; }
        () = control.cancelled() => { control.check()?; }
    }
    control.check()?;
    let mut bytes = vec![0u8; length];
    pending.component.read_into(
        &pending.spec.key,
        &mut bytes,
        &ReadInto {
            offset,
            ..Default::default()
        },
    )?;
    control
        .read_bytes
        .fetch_add(length as u64, Ordering::Release);
    control.check()?;
    if pending.spec.source_dtype == Dtype::F32 {
        return Ok(bytes);
    }
    let mut values = Vec::with_capacity(count * cols * 4);
    for raw in bytes.as_chunks::<2>().0 {
        let bits = u16::from_le_bytes(*raw);
        let value = if pending.spec.source_dtype == Dtype::BF16 {
            crate::format::decode::elem::bf16(bits)
        } else {
            crate::format::decode::elem::f16(bits)
        };
        values.extend_from_slice(&value.to_le_bytes());
    }
    Ok(values)
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
