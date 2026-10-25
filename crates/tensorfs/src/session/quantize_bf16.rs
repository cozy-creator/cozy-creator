//! Plain BF16 normalization in the existing bounded conversion worker and Writer journal.
use super::*;

const STRATEGY: &str = "bf16-rne/1";
const ROUND_TRIP_BOUND: f64 = 1.0 / 256.0;
const FIXED_BYTES: u64 = 4096;

/// An exact tensor selected by the caller's existing normalization policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizeBf16Tensor {
    pub component: String,
    pub key: String,
    pub source_dtype: Dtype,
    pub shape: Vec<u64>,
}

pub struct Normalization<'a> {
    /// Complete unchanged selection, including completed entries on resume.
    pub tensors: &'a [NormalizeBf16Tensor],
    pub max_buffer_bytes: usize,
    pub max_new_bytes: u64,
    pub control: &'a ConversionControl,
}

#[derive(Default, Debug, Serialize)]
pub struct NormalizationResult {
    pub identity: String,
    /// Newly published tensors, including zero-element tensors.
    pub converted_keys: usize,
    pub reused_keys: usize,
    /// Stored payload bytes actually requested from the source, not physical disk traffic.
    pub source_bytes_read: u64,
    /// Logical input dtype bytes, for compatibility with Runtime's previous statistic.
    pub logical_source_bytes_read: u64,
    pub new_bytes_written: u64,
    pub required_buffer_bytes: usize,
    pub worst_relative_frobenius: Option<f64>,
    pub tensors: Vec<QuantizedTensor>,
}

struct PendingNormalization<'a> {
    spec: &'a NormalizeBf16Tensor,
    component: Component,
    layout: Source,
    units: usize,
    columns: usize,
    units_per_chunk: usize,
    stored_bytes_per_unit: u64,
    logical_source_bytes: u64,
    output_bytes: u64,
    required_buffer_bytes: usize,
}

impl Writer {
    /// Normalize selected tensors to plain BF16 with the existing RNE cast. This does not
    /// commit a checkpoint. Each tensor is staged, measured, accepted, then published.
    pub async fn normalize_bf16(
        &mut self,
        source: &Checkpoint,
        options: &Normalization<'_>,
    ) -> Result<NormalizationResult> {
        if options.max_buffer_bytes == 0 {
            return fail(
                Kind::Invalid,
                "BF16 normalization needs an explicit positive buffer bound",
            );
        }
        options.control.check()?;
        let mut names = BTreeSet::new();
        let mut pending = Vec::new();
        let mut new_bytes = 0u64;
        let mut reused = 0;
        for spec in options.tensors {
            options.control.check()?;
            if !names.insert((spec.component.clone(), spec.key.clone())) {
                return fail(
                    Kind::Invalid,
                    "BF16 normalization selection repeats a tensor",
                );
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
                    "BF16 normalization source dtype or shape differs from resolved selection",
                );
            }
            let plain = matches!(tensor.body, Body::Data(_));
            if (plain && !matches!(spec.source_dtype, Dtype::F16 | Dtype::F32))
                || (!plain && !matches!(spec.source_dtype, Dtype::F16 | Dtype::BF16 | Dtype::F32))
            {
                return fail(
                    Kind::Unsupported,
                    "BF16 normalization needs plain F16/F32 or floating rowwise-encoded input",
                );
            }
            let layout = source_layout(tensor)?;
            let output_bytes = checked(Dtype::BF16.byte_len(&spec.shape))?;
            let logical_source_bytes = checked(spec.source_dtype.byte_len(&spec.shape))?;
            let elements = output_bytes / 2;
            let (units, columns, stored_bytes_per_unit, unit_memory) = match &layout {
                Source::Plain { width } => {
                    // Flat element windows also handle scalars and arbitrary plain ranks.
                    // Raw bytes, decoded F32 bytes and BF16 output are all charged.
                    (elements, 1, *width as u64, *width as u64 + 6)
                }
                Source::Rowwise { scale_width, .. } => {
                    let [rows, cols] = spec.shape.as_slice() else {
                        return fail(
                            Kind::Invalid,
                            "rowwise BF16 normalization needs rank-2 source",
                        );
                    };
                    let stored = checked(cols.checked_add(*scale_width as u64))?;
                    // Shared decoder's codes/I32/F32/serialization windows, or F32+BF16
                    // producer window, whichever is larger. Each read is complete rows.
                    let decoding = checked(
                        cols.checked_mul(9)
                            .and_then(|n| n.checked_add(*scale_width as u64 + 4)),
                    )?;
                    let encoding = checked(cols.checked_mul(6))?;
                    (
                        if elements == 0 { 0 } else { *rows },
                        *cols,
                        stored,
                        decoding.max(encoding),
                    )
                }
            };
            let room = (options.max_buffer_bytes as u64).saturating_sub(FIXED_BYTES);
            checked(units.checked_mul(stored_bytes_per_unit))?;
            let units_per_chunk = if units == 0 {
                1
            } else {
                (room / unit_memory).min(units)
            };
            let required = checked(FIXED_BYTES.checked_add(checked(
                units.min(units_per_chunk).checked_mul(unit_memory),
            )?))?;
            if units_per_chunk == 0 || required > options.max_buffer_bytes as u64 {
                let minimum =
                    checked(FIXED_BYTES.checked_add(if units == 0 { 0 } else { unit_memory }))?;
                return fail(
                    Kind::Full,
                    format!(
                        "BF16 normalization requires at least {minimum} buffer bytes, allowance is {}",
                        options.max_buffer_bytes
                    ),
                );
            }
            let platform = |n| {
                usize::try_from(n)
                    .map_err(|_| invalid("BF16 normalization geometry exceeds address space"))
            };
            platform(output_bytes)?;
            platform(logical_source_bytes)?;
            self.component_writer(&spec.component)?;
            if self.has(&spec.component, &spec.key)? {
                reused += 1;
            } else {
                new_bytes = checked(new_bytes.checked_add(output_bytes))?;
                pending.push(PendingNormalization {
                    spec,
                    component: component.clone(),
                    layout,
                    units: platform(units)?,
                    columns: platform(columns)?,
                    units_per_chunk: platform(units_per_chunk)?,
                    stored_bytes_per_unit,
                    logical_source_bytes,
                    output_bytes,
                    required_buffer_bytes: platform(required)?,
                });
            }
        }
        if new_bytes > options.max_new_bytes {
            return fail(
                Kind::Full,
                format!(
                    "BF16 normalization needs {new_bytes} output bytes, allowance is {}",
                    options.max_new_bytes
                ),
            );
        }
        let identity = crate::sha256::hex_digest(
            &serde_json::to_vec(&(
                STRATEGY,
                quantization_implementation(),
                source.id(),
                self.base_id(),
                options.tensors,
            ))
            .map_err(|e| invalid(e.to_string()))?,
        );
        if !options.tensors.is_empty() {
            self.bind_normalization(&identity, &names.into_iter().collect::<Vec<_>>())?;
        }
        let mut result = NormalizationResult {
            identity,
            reused_keys: reused,
            worst_relative_frobenius: (reused == 0).then_some(0.0),
            ..Default::default()
        };
        for pending in pending {
            options.control.check()?;
            let row = self
                .normalize_bf16_one(source, &pending, options.control)
                .await?;
            result.converted_keys += 1;
            result.source_bytes_read =
                checked(result.source_bytes_read.checked_add(row.source_bytes_read))?;
            result.logical_source_bytes_read = checked(
                result
                    .logical_source_bytes_read
                    .checked_add(pending.logical_source_bytes),
            )?;
            result.new_bytes_written =
                checked(result.new_bytes_written.checked_add(row.new_bytes_written))?;
            result.required_buffer_bytes = result
                .required_buffer_bytes
                .max(pending.required_buffer_bytes);
            if let Some(worst) = &mut result.worst_relative_frobenius {
                *worst = worst.max(row.relative_frobenius);
            }
            result.tensors.push(row);
        }
        Ok(result)
    }

    async fn normalize_bf16_one(
        &mut self,
        source: &Checkpoint,
        pending: &PendingNormalization<'_>,
        control: &ConversionControl,
    ) -> Result<QuantizedTensor> {
        let spec = pending.spec;
        let reader = FloatReader {
            checkpoint: source,
            component: &pending.component,
            component_name: &spec.component,
            key: &spec.key,
            dtype: &spec.source_dtype,
            layout: &pending.layout,
            columns: pending.columns,
        };
        let mut staged = self.scratch()?;
        let mut measured = encode::QuantizationStats::default();
        let mut source_bytes_read = 0;
        for start in (0..pending.units).step_by(pending.units_per_chunk) {
            let count = pending.units_per_chunk.min(pending.units - start);
            let values = reader.read(start, count, control).await?;
            source_bytes_read += count as u64 * pending.stored_bytes_per_unit;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(values.len() / 2)
                .map_err(|_| Failure {
                    kind: Kind::Full,
                    detail: "BF16 output window allocation failed".into(),
                })?;
            for window in values.chunks(16 << 10) {
                control.check()?;
                for raw in window.as_chunks::<4>().0 {
                    let value = f32::from_le_bytes(*raw);
                    if !value.is_finite() {
                        return fail(
                            Kind::Decode,
                            "BF16 normalization source contains nonfinite values",
                        );
                    }
                    let bits = encode::elem::bf16(value);
                    let rounded = crate::format::decode::elem::bf16(bits);
                    measured.add(value, f64::from(rounded));
                    bytes.extend_from_slice(&bits.to_le_bytes());
                }
            }
            for window in bytes.chunks(64 << 10) {
                control.check()?;
                staged
                    .file()
                    .write_all(window)
                    .map_err(|e| super::super::io_failure("BF16 staging", e))?;
            }
        }
        let bound = measured.amax * ROUND_TRIP_BOUND;
        let relative = if measured.energy > 0.0 {
            (measured.sse / measured.energy).sqrt()
        } else {
            0.0
        };
        if !measured.max_abs_error.is_finite()
            || measured.max_abs_error > bound
            || !relative.is_finite()
        {
            return fail(
                Kind::Decode,
                format!(
                    "quantization_tripwire: {}.{} bf16 max error {}, bound {bound}, relative Frobenius {relative}",
                    spec.component, spec.key, measured.max_abs_error
                ),
            );
        }
        if staged
            .file()
            .metadata()
            .map_err(|e| super::super::io_failure("BF16 staging", e))?
            .len()
            != pending.output_bytes
        {
            return fail(
                Kind::Integrity,
                "BF16 normalization produced a different byte length",
            );
        }
        staged
            .file()
            .seek(SeekFrom::Start(0))
            .map_err(|e| super::super::io_failure("BF16 staging", e))?;
        control.check()?;
        let mut checked_reader = CheckedRead {
            source: staged.file(),
            control,
        };
        let written = self.component_writer(&spec.component)?.put(
            &spec.key,
            Dtype::BF16,
            &spec.shape,
            &mut checked_reader,
        );
        if written.is_err() {
            control.check()?;
        }
        written?;
        control.written_tensors.fetch_add(1, Ordering::Release);
        Ok(QuantizedTensor {
            component: spec.component.clone(),
            key: spec.key.clone(),
            source_bytes_read,
            new_bytes_written: pending.output_bytes,
            relative_frobenius: relative,
            measured,
            fit: None,
        })
    }
}
