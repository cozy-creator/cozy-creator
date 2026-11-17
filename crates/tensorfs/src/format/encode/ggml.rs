//! `ggml.<type>`'s producer: ggml's reference quantizers (`quantize_row_<type>_ref` in
//! `ggml/src/ggml-quants.c`, llama.cpp b12466, MIT, "Copyright (c) 2023-2026 The ggml authors"),
//! ported operation for operation in f32. ggml-base builds them for baseline x86-64, so nothing
//! fuses a multiply-add; Rust never does. Without an importance matrix these are what
//! llama-quantize writes (`ggml_quantize_chunk`), so a lane's bytes equal its output.

use super::{part, value_rows, Input, QuantizationStats, Quantized};
use crate::format::decode::ggml::{Type, STRUCTS};
use crate::format::{Dtype, Error, Result};

/// The reference quantizers at llama.cpp b12466.
pub const GGML_REF_STRATEGY: &str = "ggml-ref/1";

/// The types this producer writes: Q4_K_M's mix and its fallbacks.
const TYPES: [&str; 6] = ["q8_0", "q5_0", "q5_1", "q4_k", "q5_k", "q6_k"];

/// ggml's `GROUP_MAX_EPS`.
const GROUP_MAX_EPS: f32 = 1e-15;

/// The type `scheme` names, when this producer writes it.
pub fn written(scheme: &str) -> Option<Type> {
    let name = scheme.strip_prefix("ggml.")?;
    TYPES.contains(&name).then(|| Type::by_name(name)).flatten()
}

pub(super) fn reference(input: &Input) -> Result<Quantized> {
    let scheme = input.encoding.scheme.as_str();
    let fail = |detail: &str| Error::Encode(format!("{scheme}: {detail}"));
    let ty = written(scheme).ok_or_else(|| fail("ggml-ref/1 does not write this type"))?;
    if input.encoding != &ty.encoding() {
        return Err(fail("params must be the type's block_size"));
    }
    let n = ty.block as usize;
    let size = ty.size as usize;
    let (rows, cols, bytes) = value_rows(input, n)?;
    let row = |at: usize| -> Vec<f32> {
        bytes[at * cols * 4..(at + 1) * cols * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    };
    let blocks = cols / n;
    let mut out = super::reserve::<u8>(rows * blocks * size, "ggml blocks")?;
    out.resize(rows * blocks * size, 0);
    let decode = STRUCTS
        .iter()
        .find(|s| s.name == ty.name)
        .expect("every written type decodes")
        .decode;
    let mut stats = QuantizationStats::default();
    let mut decoded = vec![0.0f32; n];
    for r in 0..rows {
        let x = row(r);
        if x.iter().any(|v| !v.is_finite()) {
            return Err(fail("a value is not finite"));
        }
        let y = &mut out[r * blocks * size..(r + 1) * blocks * size];
        for (xb, yb) in x.chunks_exact(n).zip(y.chunks_exact_mut(size)) {
            match ty.name {
                "q8_0" => q8_0(xb, yb),
                "q5_0" => q5_0(xb, yb),
                "q5_1" => q5_1(xb, yb),
                "q4_k" => k_quant(xb, yb, false),
                "q5_k" => k_quant(xb, yb, true),
                _ => q6_k(xb, yb),
            }
            decode(yb, &mut decoded);
            for (source, value) in xb.iter().zip(&decoded) {
                stats.add(*source, f64::from(*value));
            }
        }
    }
    let shape = [rows, blocks, size];
    Ok(Quantized {
        stats,
        parts: vec![part("blocks", Dtype::U8, shape, out)],
    })
}

/// ggml's `GGML_FP32_TO_FP16` (`ggml_compute_fp32_to_fp16`): round to nearest even.
fn fp16(f: f32) -> [u8; 2] {
    let scale_to_inf = f32::from_bits(0x7780_0000);
    let scale_to_zero = f32::from_bits(0x0880_0000);
    let mut base = (f.abs() * scale_to_inf) * scale_to_zero;
    let w = f.to_bits();
    let shl1_w = w.wrapping_add(w);
    let sign = w & 0x8000_0000;
    let bias = (shl1_w & 0xFF00_0000).max(0x7100_0000);
    base += f32::from_bits((bias >> 1) + 0x0780_0000);
    let bits = base.to_bits();
    let nonsign = ((bits >> 13) & 0x7C00) + (bits & 0x0FFF);
    (((sign >> 16)
        | if shl1_w > 0xFF00_0000 {
            0x7E00
        } else {
            nonsign
        }) as u16)
        .to_le_bytes()
}

fn f16(h: [u8; 2]) -> f32 {
    crate::format::decode::elem::f16(u16::from_le_bytes(h))
}

fn nearest_int(fval: f32) -> i32 {
    let val = fval + 12_582_912.0f32;
    (val.to_bits() as i32 & 0x007f_ffff) - 0x0040_0000
}

/// C's `MAX`/`MIN` (`a > b ? a : b`), never `f32::max`'s NaN rule.
fn max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

fn q8_0(x: &[f32], y: &mut [u8]) {
    let mut amax = 0.0f32;
    for v in x {
        amax = max(amax, v.abs());
    }
    let d = amax / 127.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    y[0..2].copy_from_slice(&fp16(d));
    for (q, v) in y[2..34].iter_mut().zip(x) {
        *q = (v * id).round() as i8 as u8;
    }
}

fn q5_0(x: &[f32], y: &mut [u8]) {
    let (mut amax, mut top) = (0.0f32, 0.0f32);
    for &v in x {
        if amax < v.abs() {
            amax = v.abs();
            top = v;
        }
    }
    let d = top / -16.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    y[0..2].copy_from_slice(&fp16(d));
    let mut qh = 0u32;
    for j in 0..16 {
        let xi0 = ((x[j] * id + 16.5) as i8).min(31) as u8;
        let xi1 = ((x[16 + j] * id + 16.5) as i8).min(31) as u8;
        y[6 + j] = (xi0 & 0x0F) | ((xi1 & 0x0F) << 4);
        qh |= u32::from((xi0 & 0x10) >> 4) << j;
        qh |= u32::from((xi1 & 0x10) >> 4) << (j + 16);
    }
    y[2..6].copy_from_slice(&qh.to_le_bytes());
}

fn q5_1(x: &[f32], y: &mut [u8]) {
    let (mut lo, mut hi) = (f32::MAX, -f32::MAX);
    for &v in x {
        if v < lo {
            lo = v;
        }
        if v > hi {
            hi = v;
        }
    }
    let d = (hi - lo) / 31.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    y[0..2].copy_from_slice(&fp16(d));
    y[2..4].copy_from_slice(&fp16(lo));
    let mut qh = 0u32;
    for j in 0..16 {
        let xi0 = ((x[j] - lo) * id + 0.5) as u8;
        let xi1 = ((x[16 + j] - lo) * id + 0.5) as u8;
        y[8 + j] = (xi0 & 0x0F) | ((xi1 & 0x0F) << 4);
        qh |= u32::from((xi0 & 0x10) >> 4) << j;
        qh |= u32::from((xi1 & 0x10) >> 4) << (j + 16);
    }
    y[4..8].copy_from_slice(&qh.to_le_bytes());
}

/// `make_qkx2_quants` (n = 32, `use_mad` false): the scale; `the_min` its negated min.
#[allow(clippy::too_many_arguments)]
fn make_qkx2_quants(
    nmax: i32,
    x: &[f32],
    weights: &[f32; 32],
    l_out: &mut [u8],
    the_min: &mut f32,
    rmin: f32,
    rdelta: f32,
    nstep: i32,
) -> f32 {
    let n = x.len();
    let mut laux = [0u8; 32];
    let (mut min, mut top) = (x[0], x[0]);
    let mut sum_w = weights[0];
    let mut sum_x = sum_w * x[0];
    for i in 1..n {
        if x[i] < min {
            min = x[i];
        }
        if x[i] > top {
            top = x[i];
        }
        let w = weights[i];
        sum_w += w;
        sum_x += w * x[i];
    }
    if min > 0.0 {
        min = 0.0;
    }
    if top == min {
        l_out[..n].fill(0);
        *the_min = -min;
        return 0.0;
    }
    let mut iscale = nmax as f32 / (top - min);
    let mut scale = 1.0 / iscale;
    let mut best_error = 0.0f32;
    for i in 0..n {
        let l = nearest_int(iscale * (x[i] - min));
        l_out[i] = l.clamp(0, nmax) as u8;
        let diff = scale * f32::from(l_out[i]) + min - x[i];
        best_error += weights[i] * (diff * diff);
    }
    if nstep < 1 {
        *the_min = -min;
        return scale;
    }
    for is in 0..=nstep {
        iscale = (rmin + rdelta * is as f32 + nmax as f32) / (top - min);
        let (mut sum_l, mut sum_l2, mut sum_xl) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let l = nearest_int(iscale * (x[i] - min)).clamp(0, nmax);
            laux[i] = l as u8;
            let w = weights[i];
            let lf = l as f32;
            sum_l += w * lf;
            sum_l2 += w * lf * lf;
            sum_xl += w * lf * x[i];
        }
        let d = sum_w * sum_l2 - sum_l * sum_l;
        if d > 0.0 {
            let mut this_scale = (sum_w * sum_xl - sum_x * sum_l) / d;
            let mut this_min = (sum_l2 * sum_x - sum_l * sum_xl) / d;
            if this_min > 0.0 {
                this_min = 0.0;
                this_scale = sum_xl / sum_l2;
            }
            let mut cur_error = 0.0f32;
            for i in 0..n {
                let diff = this_scale * f32::from(laux[i]) + this_min - x[i];
                cur_error += weights[i] * (diff * diff);
            }
            if cur_error < best_error {
                l_out[..n].copy_from_slice(&laux[..n]);
                best_error = cur_error;
                scale = this_scale;
                min = this_min;
            }
        }
    }
    *the_min = -min;
    scale
}

fn scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        (
            (q[j + 4] & 0xF) | ((q[j - 4] >> 6) << 4),
            (q[j + 4] >> 4) | ((q[j] >> 6) << 4),
        )
    }
}

/// `quantize_row_q4_K_ref` and, with `five`, `quantize_row_q5_K_ref`: one 256-element block.
fn k_quant(x: &[f32], y: &mut [u8], five: bool) {
    let (nmax, rmin, nstep) = if five {
        (31, -0.5f32, 15)
    } else {
        (15, -1.0f32, 20)
    };
    let mut l = [0u8; 256];
    let mut weights = [0.0f32; 32];
    let mut mins = [0.0f32; 8];
    let mut scales = [0.0f32; 8];
    let (mut max_scale, mut max_min) = (0.0f32, 0.0f32);
    for j in 0..8 {
        let xs = &x[32 * j..32 * j + 32];
        let mut sum_x2 = 0.0f32;
        for v in xs {
            sum_x2 += v * v;
        }
        let av_x = (sum_x2 / 32.0).sqrt();
        for (w, v) in weights.iter_mut().zip(xs) {
            *w = av_x + v.abs();
        }
        scales[j] = make_qkx2_quants(
            nmax,
            xs,
            &weights,
            &mut l[32 * j..32 * j + 32],
            &mut mins[j],
            rmin,
            0.1,
            nstep,
        );
        if scales[j] > max_scale {
            max_scale = scales[j];
        }
        if mins[j] > max_min {
            max_min = mins[j];
        }
    }
    let inv_scale = if max_scale > 0.0 {
        63.0 / max_scale
    } else {
        0.0
    };
    let inv_min = if max_min > 0.0 { 63.0 / max_min } else { 0.0 };
    let sc = &mut y[4..16];
    for j in 0..8 {
        let ls = (nearest_int(inv_scale * scales[j]) as u8).min(63);
        let lm = (nearest_int(inv_min * mins[j]) as u8).min(63);
        if j < 4 {
            sc[j] = ls;
            sc[j + 4] = lm;
        } else {
            sc[j + 4] = (ls & 0xF) | ((lm & 0xF) << 4);
            sc[j - 4] |= (ls >> 4) << 6;
            sc[j] |= (lm >> 4) << 6;
        }
    }
    let d16 = fp16(max_scale / 63.0);
    let dmin16 = fp16(max_min / 63.0);
    y[0..2].copy_from_slice(&d16);
    y[2..4].copy_from_slice(&dmin16);
    for j in 0..8 {
        let (s, m) = scale_min_k4(j, &y[4..16]);
        let d = f16(d16) * f32::from(s);
        if d == 0.0 {
            continue;
        }
        let dm = f16(dmin16) * f32::from(m);
        for ii in 0..32 {
            l[32 * j + ii] = nearest_int((x[32 * j + ii] + dm) / d).clamp(0, nmax) as u8;
        }
    }
    if five {
        let (qh, ql) = y[16..176].split_at_mut(32);
        qh.fill(0);
        let (mut m1, mut m2) = (1u8, 2u8);
        for (n, out) in (0..256).step_by(64).zip(ql.as_chunks_mut::<32>().0) {
            for j in 0..32 {
                let mut l1 = l[n + j];
                if l1 > 15 {
                    l1 -= 16;
                    qh[j] |= m1;
                }
                let mut l2 = l[n + j + 32];
                if l2 > 15 {
                    l2 -= 16;
                    qh[j] |= m2;
                }
                out[j] = l1 | (l2 << 4);
            }
            m1 <<= 2;
            m2 <<= 2;
        }
    } else {
        for (j, q) in (0..256).step_by(64).zip(y[16..144].as_chunks_mut::<32>().0) {
            for k in 0..32 {
                q[k] = l[j + k] | (l[j + k + 32] << 4);
            }
        }
    }
}

/// `make_qx_quants` (n = 16, nmax = 32, `rmse_type` 1, no weights): the scale.
fn make_qx_quants(x: &[f32], l_out: &mut [i8]) -> f32 {
    const NMAX: i32 = 32;
    let (mut top, mut amax) = (0.0f32, 0.0f32);
    for &v in x {
        if v.abs() > amax {
            amax = v.abs();
            top = v;
        }
    }
    if amax < GROUP_MAX_EPS {
        l_out.fill(0);
        return 0.0;
    }
    let sums = |iscale: f32| -> (f32, f32) {
        let (mut sumlx, mut suml2) = (0.0f32, 0.0f32);
        for &v in x {
            let l = nearest_int(iscale * v).clamp(-NMAX, NMAX - 1) as f32;
            let w = v * v;
            sumlx += w * v * l;
            suml2 += w * l * l;
        }
        (sumlx, suml2)
    };
    let fill = |iscale: f32, l_out: &mut [i8]| {
        for (q, &v) in l_out.iter_mut().zip(x) {
            *q = (NMAX + nearest_int(iscale * v).clamp(-NMAX, NMAX - 1)) as i8;
        }
    };
    let iscale = -(NMAX as f32) / top;
    fill(iscale, l_out);
    let (sumlx, suml2) = sums(iscale);
    let mut scale = if suml2 != 0.0 { sumlx / suml2 } else { 0.0 };
    let mut best = scale * sumlx;
    for is in -9..=9 {
        if is == 0 {
            continue;
        }
        let iscale = -(NMAX as f32 + 0.1 * is as f32) / top;
        let (sumlx, suml2) = sums(iscale);
        if suml2 > 0.0 && sumlx * sumlx > best * suml2 {
            fill(iscale, l_out);
            scale = sumlx / suml2;
            best = scale * sumlx;
        }
    }
    scale
}

fn q6_k(x: &[f32], y: &mut [u8]) {
    let mut l = [0i8; 256];
    let mut scales = [0.0f32; 16];
    let (mut max_scale, mut max_abs_scale) = (0.0f32, 0.0f32);
    for ib in 0..16 {
        let scale = make_qx_quants(&x[16 * ib..16 * ib + 16], &mut l[16 * ib..16 * ib + 16]);
        scales[ib] = scale;
        if scale.abs() > max_abs_scale {
            max_abs_scale = scale.abs();
            max_scale = scale;
        }
    }
    if max_abs_scale < GROUP_MAX_EPS {
        y.fill(0);
        y[208..210].copy_from_slice(&fp16(0.0));
        return;
    }
    let iscale = -128.0f32 / max_scale;
    let d16 = fp16(1.0 / iscale);
    y[208..210].copy_from_slice(&d16);
    for ib in 0..16 {
        y[192 + ib] = nearest_int(iscale * scales[ib]).min(127) as i8 as u8;
    }
    for j in 0..16 {
        let d = f16(d16) * f32::from(y[192 + j] as i8);
        if d == 0.0 {
            continue;
        }
        for ii in 0..16 {
            l[16 * j + ii] = (nearest_int(x[16 * j + ii] / d).clamp(-32, 31) + 32) as i8;
        }
    }
    let (ql, qh) = y[..192].split_at_mut(128);
    for (half, j) in (0..256).step_by(128).enumerate() {
        let (ql, qh) = (
            &mut ql[64 * half..64 * half + 64],
            &mut qh[32 * half..32 * half + 32],
        );
        for k in 0..32 {
            let q = |o: usize| l[j + k + o] as u8;
            ql[k] = (q(0) & 0xF) | ((q(64) & 0xF) << 4);
            ql[k + 32] = (q(32) & 0xF) | ((q(96) & 0xF) << 4);
            qh[k] = (q(0) >> 4) | ((q(32) >> 4) << 2) | ((q(64) >> 4) << 4) | ((q(96) >> 4) << 6);
        }
    }
}
