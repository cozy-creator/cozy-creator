use super::{Encoder, Input, OwnedPart, encoding, part};
use crate::format::{Dtype, Encoding, Error, Result};

/// Production Runtime's deterministic rowwise H256/16 quantizer. The revision is
/// provenance, not an extra encoding parameter: decoders only need the rotation.
pub const CONVROT_STRATEGY: &str = "int8-convrot256/1";

pub const CONVROT: Encoder = Encoder {
    scheme: "int8.convrot",
    quantize: Some(convrot),
    pack: None,
};

impl Encoding {
    pub fn int8_tensorwise() -> Encoding {
        encoding("int8.tensorwise", &[])
    }
    pub fn int8_rowwise() -> Encoding {
        encoding("int8.rowwise", &[])
    }
    pub fn int8_columnwise() -> Encoding {
        encoding("int8.columnwise", &[])
    }
    pub fn int8_convrot(rotation_size: u64) -> Encoding {
        let p = [
            ("rotation", "kron_regular_h4".into()),
            ("rotation_size", rotation_size.into()),
        ];
        encoding("int8.convrot", &p)
    }
}

/// Encode complete rows without another tensor-sized floating-point allocation.
/// Callers can split at any row boundary; neither the scale nor the rotation spans rows.
fn convrot(input: &Input) -> Result<Vec<OwnedPart>> {
    Ok(convrot_measured(input)?.parts)
}

pub(super) fn convrot_measured(input: &Input) -> Result<super::Quantized> {
    let fail = |detail: &str| Error::Encode(format!("int8.convrot: {detail}"));
    if input.encoding != &Encoding::int8_convrot(256) {
        return Err(fail(
            "int8-convrot256/1 requires kron_regular_h4, rotation_size 256",
        ));
    }
    if !matches!(input.dtype, Dtype::F32 | Dtype::F16 | Dtype::BF16) {
        return Err(fail("logical dtype must be F32, F16 or BF16"));
    }
    let [rows, cols] = input.shape else {
        return Err(fail("values must be a nonempty rank-2 matrix"));
    };
    if *rows == 0 || *cols == 0 || !cols.is_multiple_of(256) {
        return Err(fail(
            "values need nonempty rows and whole 256-column groups",
        ));
    }
    let rows = usize::try_from(*rows).map_err(|_| fail("row count overflows"))?;
    let cols = usize::try_from(*cols).map_err(|_| fail("column count overflows"))?;
    let bytes = Dtype::F32
        .byte_len(input.shape)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| fail("source byte length overflows"))?;
    let source = input
        .arrays
        .get("values")
        .ok_or_else(|| fail("no values"))?;
    if input.arrays.len() != 1 {
        return Err(fail(
            "int8-convrot256/1 accepts only values, no shared tensor_amax",
        ));
    }
    if *source.dtype != Dtype::F32 || source.shape != input.shape || source.bytes.len() != bytes {
        return Err(fail(
            "values must be exactly F32 bytes in the logical shape",
        ));
    }
    let mut weight = Vec::with_capacity(bytes / 4);
    let mut scale = Vec::with_capacity(rows * 4);
    let mut rotated = vec![0.0f64; cols];
    let mut stats = super::QuantizationStats::default();
    for row in source.bytes.chunks_exact(cols * 4) {
        for (out, raw) in rotated.iter_mut().zip(row.as_chunks::<4>().0) {
            let value = f32::from_le_bytes(*raw);
            if !value.is_finite() {
                return Err(fail("a value is not finite"));
            }
            *out = f64::from(value);
        }
        for block in rotated.as_chunks_mut::<256>().0 {
            hadamard256(block);
        }
        let mut amax = 0.0f64;
        for value in &mut rotated {
            *value /= 16.0;
            amax = amax.max(value.abs());
        }
        let row_scale = (amax / 127.0).max(1e-12) as f32;
        scale.extend_from_slice(&row_scale.to_le_bytes());
        let offset = weight.len();
        weight.extend(rotated.iter().map(|value| {
            let code = (value / f64::from(row_scale)).round_ties_even();
            stats.saturated += u64::from(code.abs() > 127.0);
            code.clamp(-127.0, 127.0) as i8 as u8
        }));
        for (codes, source) in weight[offset..]
            .as_chunks::<256>()
            .0
            .iter()
            .zip(row.as_chunks::<1024>().0)
        {
            let mut sums = codes.map(|code| i32::from(code as i8));
            hadamard256(&mut sums);
            for (sum, raw) in sums.into_iter().zip(source.as_chunks::<4>().0) {
                stats.add(
                    f32::from_le_bytes(*raw),
                    f64::from(sum) * f64::from(row_scale) / 16.0,
                );
            }
        }
    }
    Ok(super::Quantized {
        stats,
        parts: vec![
            part("weight", Dtype::I8, [rows, cols], weight),
            part("scale", Dtype::F32, [rows], scale),
        ],
    })
}

/// Unnormalized regular H256, preserving the production NumPy axis and addition order.
/// F64 is used by both producers; I32 is used for exact decoded acceptance statistics.
pub(super) fn hadamard256<T>(block: &mut [T; 256])
where
    T: Copy + std::ops::Add<Output = T> + std::ops::Sub<Output = T>,
{
    for stride in [64, 16, 4, 1] {
        for group in block.chunks_exact_mut(stride * 4) {
            for offset in 0..stride {
                let [a, b, c, d] = [0, 1, 2, 3].map(|i| group[offset + i * stride]);
                for (i, value) in [a + b + c - d, a + b - c + d, a - b + c + d, b + c + d - a]
                    .into_iter()
                    .enumerate()
                {
                    group[offset + i * stride] = value;
                }
            }
        }
    }
}
