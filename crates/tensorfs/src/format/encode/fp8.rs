use super::{Encoder, Input, QuantizationStats, Quantized, elem, encoding, part};
use crate::format::{Dtype, Encoding, Error, Result};

/// Runtime's F32 row scale, floor 1e-12, saturating E4M3 round-to-nearest-even.
pub const FP8_ROWWISE_STRATEGY: &str = "fp8-rowwise-rne/1";

pub const ROWWISE: Encoder = Encoder {
    scheme: "fp8.rowwise",
    quantize: None,
    pack: None,
};

impl Encoding {
    /// One scale for the tensor, its part stored as the producer wrote it (`F32`, `BF16` or `F16`).
    pub fn fp8_tensorwise() -> Encoding {
        encoding("fp8.tensorwise", &[])
    }
    /// A scale per row.
    pub fn fp8_rowwise() -> Encoding {
        encoding("fp8.rowwise", &[])
    }
    /// A scale per column.
    pub fn fp8_columnwise() -> Encoding {
        encoding("fp8.columnwise", &[])
    }
}

/// Complete rows; input, output, and the caller's I/O window are the only large buffers.
pub(super) fn rowwise_measured(input: &Input) -> Result<Quantized> {
    if input.encoding != &Encoding::fp8_rowwise() {
        return Err(Error::Encode(
            "fp8-rowwise-rne/1 requires parameter-free fp8.rowwise".into(),
        ));
    }
    let (rows, cols, source) = super::value_rows(input, 1)?;
    let mut weight = super::reserve(rows * cols, "FP8 weight")?;
    let mut scale = super::reserve(rows * 4, "FP8 row scales")?;
    let mut stats = QuantizationStats::default();
    for row in source.chunks_exact(cols * 4) {
        let mut amax = 0.0f32;
        for raw in row.as_chunks::<4>().0 {
            let value = f32::from_le_bytes(*raw);
            if !value.is_finite() {
                return Err(Error::Encode("fp8.rowwise: a value is not finite".into()));
            }
            amax = amax.max(value.abs());
        }
        // Both division and floor are F32, as in the historical producer.
        let row_scale = (amax / 448.0f32).max(1e-12f32);
        scale.extend_from_slice(&row_scale.to_le_bytes());
        for raw in row.as_chunks::<4>().0 {
            let value = f32::from_le_bytes(*raw);
            let normalized = value / row_scale;
            stats.saturated += u64::from(normalized.abs() > 448.0);
            let code = elem::e4m3(normalized.clamp(-448.0, 448.0));
            weight.push(code);
            // The old producer rounds the reconstructed multiply in F32 before F64 scoring.
            let reconstructed = crate::format::decode::elem::e4m3(code) * row_scale;
            stats.add(value, f64::from(reconstructed));
        }
    }
    Ok(Quantized {
        parts: vec![
            part("weight", Dtype::F8_E4M3, [rows, cols], weight),
            part("scale", Dtype::F32, [rows], scale),
        ],
        stats,
    })
}
