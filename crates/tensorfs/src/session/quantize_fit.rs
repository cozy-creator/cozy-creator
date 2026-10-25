//! Completed calibration data enters the existing conversion transaction. No model callbacks.
use super::*;

pub(super) struct PendingFit {
    component: Component,
    rows_dtype: Dtype,
    samples: usize,
    pub(super) required_buffer_bytes: usize,
    io_buffer_bytes: usize,
}

pub(super) fn parts(rows: u64, cols: u64) -> Vec<(String, Dtype, Vec<u64>)> {
    vec![
        ("weight".into(), Dtype::F4, vec![rows, cols]),
        ("scale".into(), Dtype::F8_E4M3, vec![rows, cols / 16]),
        ("scale_2".into(), Dtype::F32, vec![]),
        ("input_scale".into(), Dtype::F32, vec![]),
        ("pre_quant_scale".into(), Dtype::F32, vec![cols]),
        ("lowrank_down".into(), Dtype::BF16, vec![32, cols]),
        ("lowrank_up".into(), Dtype::BF16, vec![rows, 32]),
    ]
}

pub(super) fn preflight(
    spec: &QuantizeTensor,
    options: &Quantization<'_>,
    active: bool,
) -> Result<PendingFit> {
    let mut encoding = Encoding::nvfp4(
        encode::GlobalScale::Multiply,
        encode::NibbleOrder::LowFirst,
        crate::format::ScaleLayout::RowMajor,
    );
    encoding.scheme = "svdq.nvfp4".into();
    if spec.encoding != encoding {
        return fail(
            Kind::Unsupported,
            "SVDQ fitting requires the canonical multiply/low-first/row-major encoding",
        );
    }
    let fit = spec
        .fit
        .as_ref()
        .ok_or_else(|| invalid("SVDQ fitting requires resolved calibration references"))?;
    if fit.rows == fit.xmax {
        return fail(
            Kind::Invalid,
            "SVDQ xmax and captured rows must be distinct tensors",
        );
    }
    let calibration = options
        .calibration
        .ok_or_else(|| invalid("SVDQ fitting requires an immutable calibration checkpoint"))?;
    let component = calibration.component(&fit.component)?;
    let tensor = |key: &str| {
        component
            .tensors()
            .iter()
            .find(|t| t.key == key)
            .ok_or_else(|| {
                invalid(format!(
                    "calibration tensor {}.{key} is absent",
                    fit.component
                ))
            })
    };
    let xmax = tensor(&fit.xmax)?;
    let captured = tensor(&fit.rows)?;
    let [rows, cols] = spec.shape.as_slice() else {
        return fail(Kind::Invalid, "SVDQ fitting requires a matrix");
    };
    if xmax.dtype != Dtype::F32
        || xmax.shape != [*cols]
        || !matches!(xmax.body, Body::Data(_))
        || !matches!(captured.body, Body::Data(_))
        || !matches!(captured.dtype, Dtype::F32 | Dtype::BF16 | Dtype::F16)
    {
        return fail(
            Kind::Invalid,
            "SVDQ calibration needs plain F32 maxima [K] and floating captured rows [S,K]",
        );
    }
    let [samples, captured_cols] = captured.shape.as_slice() else {
        return fail(Kind::Invalid, "SVDQ calibration rows must be [S,K]");
    };
    if *samples == 0 || *captured_cols != *cols {
        return fail(
            Kind::Invalid,
            "SVDQ calibration rows must be nonempty and match the weight columns",
        );
    }
    let platform =
        |n| usize::try_from(n).map_err(|_| invalid("SVDQ geometry exceeds address space"));
    let core = encode::svdq_fit_workspace(platform(*rows)?, platform(*cols)?, platform(*samples)?)?;
    // A single input row is read at a time. 24*K covers raw bytes, rowwise decoder
    // I32/F32 staging and serialization while all decoded F32 inputs remain alive.
    // Metadata/part descriptors have a fixed bound. Reader/store I/O is independently bounded.
    let io_buffer_bytes = platform(checked(
        cols.checked_mul(24).and_then(|n| n.checked_add(16384)),
    )?)?;
    let required_buffer_bytes = core
        .checked_add(io_buffer_bytes)
        .ok_or_else(|| invalid("SVDQ workspace size overflows"))?;
    if active && required_buffer_bytes > options.max_buffer_bytes {
        return fail(
            Kind::Full,
            format!(
                "SVDQ fitting requires {required_buffer_bytes} buffer bytes, allowance is {}",
                options.max_buffer_bytes
            ),
        );
    }
    Ok(PendingFit {
        component: component.clone(),
        rows_dtype: captured.dtype.clone(),
        samples: platform(*samples)?,
        required_buffer_bytes,
        io_buffer_bytes,
    })
}

fn floats(len: usize) -> Result<Vec<f32>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| Failure {
        kind: Kind::Full,
        detail: "SVDQ input allocation failed within the admitted workspace".into(),
    })?;
    Ok(out)
}

async fn read_calibration(
    source: &Checkpoint,
    pending: &PendingFit,
    fit: &QuantizeFit,
    key: &str,
    dtype: &Dtype,
    rows: usize,
    cols: usize,
    control: &ConversionControl,
) -> Result<(Vec<f32>, u64)> {
    let width = if *dtype == Dtype::F32 { 4 } else { 2 };
    let mut values = floats(
        rows.checked_mul(cols)
            .ok_or_else(|| invalid("calibration size overflows"))?,
    )?;
    let mut read_bytes = 0;
    for row in 0..rows {
        let bytes = read_named_part(
            source,
            &pending.component,
            &fit.component,
            key,
            None,
            (row * cols * width) as u64,
            cols * width,
            control,
        )
        .await?;
        read_bytes += bytes.len() as u64;
        if *dtype == Dtype::F32 {
            values.extend(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b)),
            );
        } else {
            values.extend(bytes.as_chunks::<2>().0.iter().map(|b| {
                let bits = u16::from_le_bytes(*b);
                if *dtype == Dtype::BF16 {
                    crate::format::decode::elem::bf16(bits)
                } else {
                    crate::format::decode::elem::f16(bits)
                }
            }));
        }
        control.check()?;
    }
    Ok((values, read_bytes))
}

impl Writer {
    pub(super) async fn quantize_fit(
        &mut self,
        source: &Checkpoint,
        pending: &Pending<'_>,
        options: &Quantization<'_>,
    ) -> Result<QuantizedTensor> {
        let spec = pending.spec;
        let fit = spec.fit.as_ref().expect("preflight fit");
        let plan = pending.fit.as_ref().expect("preflight fit plan");
        let calibration = options.calibration.expect("preflight calibration");
        let rows = spec.shape[0] as usize;
        let cols = spec.shape[1] as usize;
        let mut weight = floats(
            rows.checked_mul(cols)
                .ok_or_else(|| invalid("weight size overflows"))?,
        )?;
        let mut source_bytes_read = 0;
        for row in 0..rows {
            let bytes = read_window(source, pending, row, 1, options.control).await?;
            weight.extend(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b)),
            );
            source_bytes_read += pending.row_source_bytes;
        }
        let (xmax, xmax_bytes) = read_calibration(
            calibration,
            plan,
            fit,
            &fit.xmax,
            &Dtype::F32,
            1,
            cols,
            options.control,
        )
        .await?;
        let (captured, captured_bytes) = read_calibration(
            calibration,
            plan,
            fit,
            &fit.rows,
            &plan.rows_dtype,
            plan.samples,
            cols,
            options.control,
        )
        .await?;
        let mut check = || {
            options
                .control
                .check()
                .map_err(|e| crate::format::Error::Encode(e.to_string()))
        };
        let fitted = encode::svdq_fit(
            &weight,
            &xmax,
            &captured,
            rows,
            cols,
            plan.samples,
            fit.seed,
            options.max_buffer_bytes - plan.io_buffer_bytes,
            &mut check,
        );
        // Preserve the existing typed cancellation even though the format core has no session dependency.
        options.control.check()?;
        let fitted = fitted?;
        drop((weight, xmax, captured));
        let measured = fitted.stats;
        let relative = if measured.energy > 0.0 {
            (measured.sse / measured.energy).sqrt()
        } else {
            0.0
        };
        let bound = fitted.residual.amax * 0.25;
        if !fitted.residual.max_abs_error.is_finite()
            || fitted.residual.max_abs_error > bound
            || !relative.is_finite()
            || options
                .max_relative_frobenius
                .is_some_and(|limit| relative > limit)
        {
            return fail(
                Kind::Decode,
                format!(
                    "quantization_tripwire: {}.{} residual max error {}, bound {bound}, whole-weight relative Frobenius {relative}",
                    spec.component, spec.key, fitted.residual.max_abs_error
                ),
            );
        }
        if fitted.parts.len() != pending.parts.len() {
            return fail(
                Kind::Integrity,
                "SVDQ fitter changed its resolved part count",
            );
        }
        let mut staged = Vec::new();
        for (part, (role, dtype, shape)) in fitted.parts.into_iter().zip(&pending.parts) {
            options.control.check()?;
            if part.role != *role
                || part.dtype != *dtype
                || part.shape != *shape
                || Some(part.bytes.len() as u64) != dtype.byte_len(shape)
            {
                return fail(
                    Kind::Integrity,
                    "SVDQ fitter changed its resolved part geometry",
                );
            }
            let mut file = self.scratch()?;
            // Bound cancellation latency during staging as well as during fitting.
            for window in part.bytes.chunks(64 << 10) {
                options.control.check()?;
                file.file()
                    .write_all(window)
                    .map_err(|e| super::super::io_failure("SVDQ staging", e))?;
            }
            file.file()
                .seek(SeekFrom::Start(0))
                .map_err(|e| super::super::io_failure("SVDQ staging", e))?;
            staged.push(file);
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
            source_bytes_read,
            new_bytes_written: pending.output_bytes,
            relative_frobenius: relative,
            measured,
            fit: Some(FitDetails {
                strategy: spec.strategy.clone(),
                alpha: fitted.alpha,
                residual: fitted.residual,
                calibration_error: fitted.calibration_error,
                plain_nvfp4_error: fitted.plain_nvfp4_error,
                calibration_bytes_read: xmax_bytes + captured_bytes,
                required_buffer_bytes: plan.required_buffer_bytes,
            }),
        })
    }
}
