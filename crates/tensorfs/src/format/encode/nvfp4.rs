//! `nvfp4`'s encoders. `quantize` is ModelOpt's round to nearest (`NVFP4QTensor.quantize`, byte for
//! byte); `pack` lays out a calibrated producer's codes and scales. Both write either layout, a
//! global scale per tensor or per matrix, and AWQ's `pre_quant_scale`.
//! The explicit `nvfp4-min-sse/1` strategy separately preserves Runtime's bounded
//! three-candidate producer in its row-major, low-first, multiply layout.

use super::elem::e2m1;
use super::{Input, OwnedPart, encoding};
use crate::format::{Dtype, Encoding, Error, Result, ScaleLayout};

/// Which nibble of a byte holds an F4 pair's even element: ModelOpt's low, ComfyUI's high.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NibbleOrder {
    LowFirst,
    HighFirst,
}

/// How nvfp4's global scales, `scale_2` and `input_scale`, apply: multiplied (ModelOpt,
/// ComfyUI) or divided, being reciprocals (compressed-tensors' `weight_global_scale` and
/// `input_global_scale`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalScale {
    Multiply,
    Divide,
}

impl NibbleOrder {
    pub(crate) fn name(self) -> &'static str {
        match self {
            NibbleOrder::LowFirst => "low_first",
            NibbleOrder::HighFirst => "high_first",
        }
    }
}

impl GlobalScale {
    pub(crate) fn name(self) -> &'static str {
        match self {
            GlobalScale::Multiply => "multiply",
            GlobalScale::Divide => "divide",
        }
    }
}

impl Encoding {
    pub fn nvfp4(global: GlobalScale, nibbles: NibbleOrder, layout: ScaleLayout) -> Encoding {
        let p = [
            ("block_size", 16u64.into()),
            ("global_scale", global.name().into()),
            ("nibble_order", nibbles.name().into()),
            ("scale_layout", layout.name().into()),
        ];
        encoding("nvfp4", &p)
    }
}

/// Runtime's three-candidate minimum-error producer; distinct from the stored scheme.
pub const MIN_SSE_STRATEGY: &str = "nvfp4-min-sse/1";

fn fail(detail: impl std::fmt::Display) -> Error {
    Error::Encode(format!("nvfp4: {detail}"))
}

/// Production Runtime's min-SSE rule, with a supplied whole-tensor maximum. The only
/// floating scratch is one 16-value block. Emit packed bytes directly: the legacy
/// RTN layout helper takes an expanded tensor-sized code vector.
pub(super) fn min_sse(input: &Input) -> Result<super::Quantized> {
    if input.encoding
        != &Encoding::nvfp4(
            GlobalScale::Multiply,
            NibbleOrder::LowFirst,
            ScaleLayout::RowMajor,
        )
    {
        return Err(fail(
            "nvfp4-min-sse/1 requires multiply, low_first, row_major and block_size 16",
        ));
    }
    if !matches!(input.dtype, Dtype::F32 | Dtype::F16 | Dtype::BF16) {
        return Err(fail("logical dtype must be F32, F16 or BF16"));
    }
    let [rows, cols] = input.shape else {
        return Err(fail("values must be a nonempty rank-2 matrix"));
    };
    if *rows == 0 || *cols == 0 || !cols.is_multiple_of(16) {
        return Err(fail("values need nonempty rows and whole 16-column blocks"));
    }
    let bytes = Dtype::F32
        .byte_len(input.shape)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| fail("source byte length overflows"))?;
    let source = input
        .arrays
        .get("values")
        .ok_or_else(|| fail("no values"))?;
    if *source.dtype != Dtype::F32 || source.shape != input.shape || source.bytes.len() != bytes {
        return Err(fail(
            "values must be exactly F32 bytes in the logical shape",
        ));
    }
    let maximum = input
        .arrays
        .get("tensor_amax")
        .ok_or_else(|| fail("nvfp4-min-sse/1 requires the whole-tensor tensor_amax"))?;
    if *maximum.dtype != Dtype::F64
        || !maximum.shape.is_empty()
        || maximum.bytes.len() != 8
        || input.arrays.len() != 2
    {
        return Err(fail(
            "tensor_amax must be a scalar F64; only values and tensor_amax are accepted",
        ));
    }
    let maximum = f64::from_le_bytes(maximum.bytes.try_into().expect("checked scalar length"));
    if !maximum.is_finite() || maximum < 0.0 {
        return Err(fail("tensor_amax must be finite and nonnegative"));
    }
    let global = min_sse_global(maximum)?;
    let mut weight = Vec::with_capacity(bytes / 8);
    let mut scale = Vec::with_capacity(bytes / 64);
    let mut stats = super::QuantizationStats::default();
    for raw in source.bytes.as_chunks::<64>().0 {
        let mut block = [0.0f32; 16];
        for (value, raw) in block.iter_mut().zip(raw.as_chunks::<4>().0) {
            *value = f32::from_le_bytes(*raw);
            if !value.is_finite() || f64::from(value.abs()) > maximum {
                return Err(fail(
                    "a value is not finite or exceeds whole-tensor tensor_amax",
                ));
            }
        }
        let (chosen, codes, saturated) = min_sse_block(&block, global);
        let effective = crate::format::decode::elem::e4m3(chosen) * global;
        stats.saturated += saturated;
        for (value, code) in block.into_iter().zip(codes) {
            let reconstructed = crate::format::decode::elem::e2m1(code) * effective;
            stats.add(value, f64::from(reconstructed));
        }
        scale.push(chosen);
        weight.extend(
            codes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| pair[0] | pair[1] << 4),
        );
    }
    Ok(super::Quantized {
        stats,
        parts: vec![
            OwnedPart {
                role: "weight".into(),
                dtype: Dtype::F4,
                shape: input.shape.to_vec(),
                bytes: weight,
            },
            OwnedPart {
                role: "scale".into(),
                dtype: Dtype::F8_E4M3,
                shape: vec![*rows, *cols / 16],
                bytes: scale,
            },
            OwnedPart {
                role: "scale_2".into(),
                dtype: Dtype::F32,
                shape: vec![],
                bytes: global.to_le_bytes().to_vec(),
            },
        ],
    })
}

/// Shared checked global scale for the production min-SSE encoder.
pub(super) fn min_sse_global(maximum: f64) -> Result<f32> {
    if !maximum.is_finite() || maximum < 0.0 {
        return Err(fail("tensor_amax must be finite and nonnegative"));
    }
    let global = if maximum == 0.0 {
        1.0
    } else {
        (maximum / (448.0 * 6.0)) as f32
    };
    if !global.is_finite() || global <= 0.0 {
        return Err(fail(
            "tensor_amax yields an unrepresentable positive F32 global scale",
        ));
    }
    Ok(global)
}

/// Exact production block law, shared with the fitted SVDQ residual. Caller
/// supplies a finite block and positive checked whole-tensor global scale.
pub(super) fn min_sse_block(block: &[f32; 16], global: f32) -> (u8, [u8; 16], u64) {
    let magnitudes: [f32; 127] =
        std::array::from_fn(|i| crate::format::decode::elem::e4m3(i as u8));
    let amax = block.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    // The division and the coverage recheck intentionally round in F32.
    let need = amax / (6.0f32 * global);
    let mut anchor = magnitudes.partition_point(|s| *s < need).min(126);
    for _ in 0..3 {
        let effective = magnitudes[anchor] * global;
        if anchor == 126 || amax <= effective * 6.0f32 {
            break;
        }
        anchor += 1;
    }
    let mut best: Option<(f64, u8, [u8; 16])> = None;
    for candidate in [anchor.saturating_sub(1), anchor, (anchor + 1).min(126)] {
        let effective = magnitudes[candidate] * global;
        let codes = block.map(|value| {
            e2m1(if effective > 0.0 {
                value / effective
            } else {
                0.0
            })
        });
        let errors: [f64; 16] = std::array::from_fn(|i| {
            let reconstructed = crate::format::decode::elem::e2m1(codes[i]) * effective;
            let difference = f64::from(reconstructed - block[i]);
            difference * difference
        });
        // NumPy's contiguous 16-element sum: eight lanes, then a balanced tree.
        // Preserve this order because it can decide which scale wins a close tie.
        let lanes: [f64; 8] = std::array::from_fn(|i| errors[i] + errors[i + 8]);
        let sse = ((lanes[0] + lanes[1]) + (lanes[2] + lanes[3]))
            + ((lanes[4] + lanes[5]) + (lanes[6] + lanes[7]));
        if best.as_ref().is_none_or(|(error, _, _)| sse < *error) {
            best = Some((sse, candidate as u8, codes));
        }
    }
    let (_, chosen, codes) = best.expect("three candidates");
    let effective = magnitudes[chosen as usize] * global;
    let saturated = block
        .iter()
        .filter(|value| value.abs() > effective * 6.0f32)
        .count() as u64;
    (chosen, codes, saturated)
}
