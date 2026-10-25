//! Runtime's fixed-codebook W4A8 producer, distinct from Kitchen's dtype-rounded fitter.
//! Rows are independent. Only one F64 rotated row and its F64 group scales are live;
//! encoded output is returned to the existing Writer's bounded staging loop.

use super::{
    Input, QuantizationStats, Quantized, affine, comfy::LUT, elem, int8::hadamard256, part,
};
use crate::format::{Dtype, Encoding, Error, Result};

pub const W4A8_STRATEGY: &str = "w4a8-convrot256-fixed/1";

// NumPy's contiguous length-16 F64 reduction: eight lanes, then the same balanced tree.
// No fused multiply/add: the producer materializes products before this reduction.
fn sum16(values: [f64; 16]) -> f64 {
    let r: [f64; 8] = std::array::from_fn(|i| values[i] + values[i + 8]);
    ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]))
}

fn nearest(book: &[f64; 16], value: f64) -> usize {
    let mut best = 0;
    let mut distance = (value - book[0]).abs();
    for (index, level) in book.iter().enumerate().skip(1) {
        let candidate = (value - level).abs();
        if candidate < distance {
            best = index;
            distance = candidate;
        }
    }
    best
}

pub(super) fn fixed_measured(input: &Input) -> Result<Quantized> {
    let fail = |detail: &str| Error::Encode(format!("w4a8.convrot: {detail}"));
    if input.encoding != &Encoding::w4a8_convrot(256) {
        return Err(fail(
            "w4a8-convrot256-fixed/1 requires group_size 16, kron_regular_h4, rotation_size 256",
        ));
    }
    let (rows, cols, source) = super::value_rows(input, 256)?;
    let groups = cols / 16;
    let mut weight = super::reserve(rows * (cols / 2), "W4A8 weight")?;
    let mut relative = super::reserve(rows * groups, "W4A8 group scales")?;
    let mut scale = super::reserve(rows * 4, "W4A8 row scales")?;
    let mut book_bytes = super::reserve(64, "W4A8 codebook")?;
    for value in LUT {
        book_bytes.extend_from_slice(&value.to_le_bytes());
    }
    let book = LUT.map(f64::from);
    let mut rotated = super::reserve(cols, "W4A8 rotated row")?;
    rotated.resize(cols, 0.0f64);
    let mut group_scale = super::reserve(groups, "W4A8 fitted group scales")?;
    group_scale.resize(groups, 0.0f64);
    let mut stats = QuantizationStats::default();
    for row in source.chunks_exact(cols * 4) {
        for (out, raw) in rotated.iter_mut().zip(row.as_chunks::<4>().0) {
            let value = f32::from_le_bytes(*raw);
            if !value.is_finite() {
                return Err(fail("a value is not finite"));
            }
            *out = f64::from(value);
        }
        for block in rotated.as_chunks_mut::<256>().0 {
            hadamard256(block);
            for value in block {
                *value /= 16.0;
            }
        }
        let mut amax = 0.0f64;
        for (group, fitted) in rotated.as_chunks::<16>().0.iter().zip(&mut group_scale) {
            let mut gs = group.iter().fold(0.0f64, |a, x| a.max(x.abs())).max(1e-8);
            let mut indices = group.map(|value| nearest(&book, value / gs));
            for _ in 0..2 {
                let levels = indices.map(|index| book[index]);
                let numerator = sum16(std::array::from_fn(|i| group[i] * levels[i]));
                let denominator = sum16(levels.map(|value| value * value)).max(1e-8);
                gs = (numerator / denominator).max(1e-8);
                indices = group.map(|value| nearest(&book, value / gs));
            }
            *fitted = gs;
            for index in indices {
                amax = amax.max((book[index] * gs).abs());
            }
        }
        let row_scale = (amax / 127.0).max(1e-8) as f32;
        if !row_scale.is_finite() {
            return Err(fail("row scale is not finite in F32"));
        }
        scale.extend_from_slice(&row_scale.to_le_bytes());
        // Reuse one integer H256 block for decoded acceptance statistics; no full q8 copy.
        let mut reconstructed = [0i32; 256];
        for (index, (group, gs)) in rotated
            .as_chunks::<16>()
            .0
            .iter()
            .zip(&group_scale)
            .enumerate()
        {
            let rel = elem::e4m3((*gs / f64::from(row_scale)) as f32);
            relative.push(rel);
            let rel_value = crate::format::decode::elem::e4m3(rel);
            // F32 multiply and RNE precede the integer grid, exactly as NumPy's original.
            let levels =
                LUT.map(|value| (value * rel_value).round_ties_even().clamp(-127.0, 127.0));
            let levels64 = levels.map(f64::from);
            let mut codes = [0u8; 16];
            for (column, value) in group.iter().enumerate() {
                let target = value / f64::from(row_scale);
                stats.saturated += u64::from(target.abs() > 127.5);
                let code = nearest(&levels64, target);
                codes[column] = code as u8;
                reconstructed[(index % 16) * 16 + column] = levels[code] as i32;
            }
            for pair in codes.as_chunks::<2>().0 {
                weight.push(affine::nibble_pair(pair[0], pair[1]));
            }
            if index % 16 == 15 {
                hadamard256(&mut reconstructed);
                let start = (index / 16) * 256 * 4;
                for (sum, raw) in reconstructed
                    .iter()
                    .zip(row[start..start + 1024].as_chunks::<4>().0)
                {
                    stats.add(
                        f32::from_le_bytes(*raw),
                        f64::from(*sum) * f64::from(row_scale) / 16.0,
                    );
                }
            }
        }
    }
    Ok(Quantized {
        parts: vec![
            part("weight", Dtype::I8, [rows, cols / 2], weight),
            part("group_scale", Dtype::F8_E4M3, [rows, groups], relative),
            part("scale", Dtype::F32, [rows], scale),
            part("codebook", Dtype::F32, [16], book_bytes),
        ],
        stats,
    })
}
