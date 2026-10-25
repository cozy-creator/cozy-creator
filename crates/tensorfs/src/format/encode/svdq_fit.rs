//! The complete, bounded CPU producer for `svdq.nvfp4`.
//!
//! The caller owns immutable row-major F32 inputs. Their bytes count against this
//! operation's grant. File read/decode staging must be charged separately by Writer.
//! Numerical matrices and encoded parts are fallibly allocated once and reused. The
//! patched faer dependency uses one-column Householder blocks, and q <= 48 stays in
//! its bidiagonal QR base case; no unreported GEMM packing cache is entered.

use faer::diag::DiagMut;
use faer::dyn_stack::{MemBuffer, MemStack, StackReq};
use faer::linalg::{householder, qr::no_pivoting::factor, svd};
use faer::prelude::ReborrowMut;
use faer::{Accum, Conj, MatMut, MatRef, Par};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use rand_distr::{Distribution, StandardNormal};

use super::super::{OwnedPart, QuantizationStats, elem, nvfp4};
use crate::format::{Dtype, Error, Result};

pub const SVDQ_FIT_STRATEGY: &str = "svdq-nvfp4-r32-faer/1";
pub const SVDQ_NUMERICAL_IDENTITY: &str = concat!(
    "faer=0.24.4;tensorfs-householder-block1/1;f32;seq;qr-bidiag-threshold128;",
    "rank32;oversample16;power4;alpha0,.2,.3,.4,.5,.6,.7,.8;",
    "rand=0.9.5;rand_chacha=0.9.0;ChaCha20Rng::seed_from_u64;",
    "rand_distr=0.5.1;StandardNormal<f32>;sketch-column-major;",
    "bf16-rne;canonical-smooth=recip(stored-pre);final-residual-ordered-f32;",
    "faer-nano-gemm-16cubed;score-f64-norms;final-activation-mul-pre;logical-decode-f32"
);
const RANK: usize = 32;
const Q: usize = RANK + 16;
const TILE: usize = 16;
const LEVELS: f32 = 2688.0;
const ALPHAS: [f32; 8] = [0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];

pub struct SvdqFitted {
    pub parts: Vec<OwnedPart>,
    /// The whole logical weight, separate from the residual encoder's tripwire.
    pub stats: QuantizationStats,
    pub residual: QuantizationStats,
    pub alpha: f32,
    pub calibration_error: f64,
    pub plain_nvfp4_error: f64,
    pub required_buffer_bytes: usize,
}

fn fail(detail: impl std::fmt::Display) -> Error {
    Error::Encode(format!("{SVDQ_FIT_STRATEGY}: {detail}"))
}
fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| fail("workspace byte count overflows"))
}
fn mul(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b)
        .filter(|n| *n <= isize::MAX as usize)
        .ok_or_else(|| fail("workspace shape or byte count overflows"))
}
fn zeros<T: Default + Clone>(n: usize) -> Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(n)
        .map_err(|_| fail(format!("cannot allocate {n} workspace elements")))?;
    out.resize(n, T::default());
    Ok(out)
}
fn finite(values: &[f32], what: &str) -> Result<()> {
    if values.iter().any(|x| !x.is_finite()) {
        return Err(fail(format!("nonfinite {what}")));
    }
    Ok(())
}
fn geometry(rows: usize, cols: usize, samples: usize) -> Result<usize> {
    if rows < RANK || cols == 0 || !cols.is_multiple_of(256) || samples == 0 {
        return Err(fail(
            "requires N >= 32, K a positive multiple of 256 and nonempty calibration rows",
        ));
    }
    mul(rows, cols)?;
    mul(samples, cols)?;
    mul(samples, rows)?;
    Ok(rows.min(cols).min(Q))
}
fn svd_params() -> faer::Spec<svd::SvdParams, f32> {
    let mut params = faer::Spec::<svd::SvdParams, f32>::default();
    // q <= 48 must never enter divide-and-conquer's packed GEMM path.
    params.recursion_threshold = 128;
    params
}
fn scratch(rows: usize, cols: usize, q: usize) -> Result<StackReq> {
    // This constant deliberately fails to compile against the unpatched crate.
    // Stock faer's StackReq does not include its internal GEMM packing cache.
    let _ = svd::TENSORFS_BOUNDED_SVD_REVISION;
    let req = StackReq::any_of(&[
        factor::qr_in_place_scratch::<f32>(rows, q, 1, Par::Seq, Default::default()),
        householder::apply_block_householder_sequence_on_the_left_in_place_scratch::<f32>(
            rows, 1, q,
        ),
        svd::svd_scratch::<f32>(
            q,
            cols,
            svd::ComputeSvdVectors::Thin,
            svd::ComputeSvdVectors::Thin,
            Par::Seq,
            svd_params(),
        ),
    ]);
    req.layout()
        .map_err(|_| fail("faer scratch requirement overflows"))?;
    Ok(req)
}

/// A conservative peak byte bound including the immutable F32 input slices,
/// all reusable matrices, library scratch, encoded payloads and metadata. It
/// excludes only the caller's explicitly separate bounded file-I/O staging.
pub fn svdq_fit_workspace(rows: usize, cols: usize, samples: usize) -> Result<usize> {
    let q = geometry(rows, cols, samples)?;
    let nk = mul(rows, cols)?;
    let sk = mul(samples, cols)?;
    let nq = mul(rows, q)?;
    let kq = mul(cols, q)?;
    let nr = mul(rows, RANK)?;
    let kr = mul(cols, RANK)?;
    let st = mul(samples, TILE)?;
    // Inputs; smoothed weight; two basis buffers; reduced/sketch and V; small U;
    // singular values and one-row Householder coefficients; current/best factors;
    // smooth, stored pre and channel maxima; quantized activations; reference;
    // one residual tile; current residual and low-rank output tiles; projected rows.
    let counts = [
        nk,
        sk,
        cols,
        nk,
        nq,
        nq,
        kq,
        kq,
        mul(q, q)?,
        q,
        q,
        nr,
        nr,
        kr,
        kr,
        cols,
        cols,
        cols,
        sk,
        mul(samples, rows)?,
        mul(TILE, cols)?,
        st,
        st,
        mul(samples, RANK)?,
    ];
    let mut bytes = 64 * 1024; // bounded stack/Vec/part/shape/string metadata and RNG state
    for count in counts {
        bytes = add(bytes, mul(count, 4)?)?;
    }
    // Norm/error accumulators, and all seven final encoded parts.
    bytes = add(bytes, mul(mul(samples, 2)?, 8)?)?;
    for count in [nk / 2, nk / 16, 8, mul(cols, 4)?, mul(nr, 2)?, mul(kr, 2)?] {
        bytes = add(bytes, count)?;
    }
    add(bytes, scratch(rows, cols, q)?.unaligned_bytes_required())
}

fn row(values: &[f32], rows: usize, cols: usize) -> MatRef<'_, f32> {
    MatRef::from_row_major_slice(values, rows, cols)
}
fn row_mut(values: &mut [f32], rows: usize, cols: usize) -> MatMut<'_, f32> {
    MatMut::from_row_major_slice_mut(values, rows, cols)
}
fn col(values: &[f32], rows: usize, cols: usize) -> MatRef<'_, f32> {
    MatRef::from_column_major_slice(values, rows, cols)
}
fn col_mut(values: &mut [f32], rows: usize, cols: usize) -> MatMut<'_, f32> {
    MatMut::from_column_major_slice_mut(values, rows, cols)
}

/// <= 16^3 operations selects faer's allocation-free nano-GEMM path even on
/// machines whose regular GEMM backend keeps a large thread-local packing cache.
fn product(
    mut dst: MatMut<'_, f32>,
    lhs: MatRef<'_, f32>,
    rhs: MatRef<'_, f32>,
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    for i in (0..dst.nrows()).step_by(TILE) {
        check()?;
        let m = (dst.nrows() - i).min(TILE);
        for j in (0..dst.ncols()).step_by(TILE) {
            let n = (dst.ncols() - j).min(TILE);
            for k in (0..lhs.ncols()).step_by(TILE) {
                if k.is_multiple_of(256) {
                    check()?;
                }
                let depth = (lhs.ncols() - k).min(TILE);
                faer::linalg::matmul::matmul(
                    dst.rb_mut().submatrix_mut(i, j, m, n),
                    if k == 0 { Accum::Replace } else { Accum::Add },
                    lhs.submatrix(i, k, m, depth),
                    rhs.submatrix(k, j, depth, n),
                    1.0,
                    Par::Seq,
                );
            }
        }
    }
    Ok(())
}

struct Work {
    a: Vec<f32>,
    y: Vec<f32>,
    basis: Vec<f32>,
    z: Vec<f32>,
    v: Vec<f32>,
    u: Vec<f32>,
    sigma: Vec<f32>,
    hh: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
    best_up: Vec<f32>,
    best_down: Vec<f32>,
    smooth: Vec<f32>,
    pre: Vec<f32>,
    wmax: Vec<f32>,
    xq: Vec<f32>,
    reference: Vec<f32>,
    residual_tile: Vec<f32>,
    prediction: Vec<f32>,
    branch: Vec<f32>,
    projected: Vec<f32>,
    norms: Vec<f64>,
    errors: Vec<f64>,
    scratch: MemBuffer,
}
impl Work {
    fn new(n: usize, k: usize, s: usize, q: usize) -> Result<Self> {
        Ok(Self {
            a: zeros(n * k)?,
            y: zeros(n * q)?,
            basis: zeros(n * q)?,
            z: zeros(k * q)?,
            v: zeros(k * q)?,
            u: zeros(q * q)?,
            sigma: zeros(q)?,
            hh: zeros(q)?,
            up: zeros(n * RANK)?,
            down: zeros(RANK * k)?,
            best_up: zeros(n * RANK)?,
            best_down: zeros(RANK * k)?,
            smooth: zeros(k)?,
            pre: zeros(k)?,
            wmax: zeros(k)?,
            xq: zeros(s * k)?,
            reference: zeros(s * n)?,
            residual_tile: zeros(TILE * k)?,
            prediction: zeros(s * TILE)?,
            branch: zeros(s * TILE)?,
            projected: zeros(s * RANK)?,
            norms: zeros(s)?,
            errors: zeros(s)?,
            scratch: MemBuffer::try_new(scratch(n, k, q)?)
                .map_err(|_| fail("cannot allocate faer scratch"))?,
        })
    }
    fn orthogonalize(
        &mut self,
        n: usize,
        q: usize,
        check: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        finite(&self.y, "subspace iterate")?;
        check()?;
        factor::qr_in_place(
            col_mut(&mut self.y, n, q),
            col_mut(&mut self.hh, 1, q),
            Par::Seq,
            MemStack::new(&mut self.scratch),
            Default::default(),
        );
        check()?;
        self.basis.fill(0.0);
        for j in 0..q {
            self.basis[j * n + j] = 1.0;
        }
        householder::apply_block_householder_sequence_on_the_left_in_place_with_conj(
            col(&self.y, n, q),
            col(&self.hh, 1, q),
            Conj::No,
            col_mut(&mut self.basis, n, q),
            Par::Seq,
            MemStack::new(&mut self.scratch),
        );
        check()?;
        finite(&self.basis, "orthogonal basis")
    }
    fn factors(
        &mut self,
        n: usize,
        k: usize,
        q: usize,
        seed: u64,
        check: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        for chunk in self.z.chunks_mut(4096) {
            check()?;
            for x in chunk {
                *x = StandardNormal.sample(&mut rng);
            }
        }
        product(
            col_mut(&mut self.y, n, q),
            row(&self.a, n, k),
            col(&self.z, k, q),
            check,
        )?;
        for _ in 0..4 {
            self.orthogonalize(n, q, check)?;
            product(
                col_mut(&mut self.z, k, q),
                row(&self.a, n, k).transpose(),
                col(&self.basis, n, q),
                check,
            )?;
            finite(&self.z, "subspace projection")?;
            product(
                col_mut(&mut self.y, n, q),
                row(&self.a, n, k),
                col(&self.z, k, q),
                check,
            )?;
        }
        self.orthogonalize(n, q, check)?;
        product(
            col_mut(&mut self.z, k, q),
            row(&self.a, n, k).transpose(),
            col(&self.basis, n, q),
            check,
        )?;
        finite(&self.z, "reduced SVD input")?;
        check()?;
        svd::svd(
            col(&self.z, k, q).transpose(),
            DiagMut::from_slice_mut(&mut self.sigma),
            Some(col_mut(&mut self.u, q, q)),
            Some(col_mut(&mut self.v, k, q)),
            Par::Seq,
            MemStack::new(&mut self.scratch),
            svd_params(),
        )
        .map_err(|e| fail(format!("reduced SVD failed: {e:?}")))?;
        check()?;
        finite(&self.sigma, "singular values")?;
        finite(&self.u, "left singular vectors")?;
        finite(&self.v, "right singular vectors")?;
        product(
            row_mut(&mut self.up, n, RANK),
            col(&self.basis, n, q),
            col(&self.u, q, q).submatrix(0, 0, q, RANK),
            check,
        )?;
        for r in 0..RANK {
            check()?;
            let mut pivot = 0;
            for j in 1..k {
                if self.v[r * k + j].abs() > self.v[r * k + pivot].abs() {
                    pivot = j;
                }
            }
            let sign = if self.v[r * k + pivot] < 0.0 {
                -1.0
            } else {
                1.0
            };
            for i in 0..n {
                self.up[i * RANK + r] = rounded(self.up[i * RANK + r] * self.sigma[r] * sign)?;
            }
            for j in 0..k {
                self.down[r * k + j] = rounded(self.v[r * k + j] * sign / self.smooth[j])?;
            }
        }
        Ok(())
    }
}
fn rounded(x: f32) -> Result<f32> {
    if !x.is_finite() {
        return Err(fail("nonfinite factor or BF16 operand"));
    }
    let y = crate::format::decode::elem::bf16(elem::bf16(x));
    if !y.is_finite() {
        return Err(fail("factor or operand overflows BF16"));
    }
    Ok(y)
}

/// Explicit F32 operation order, also used by the independent packing oracle.
fn residual_value(
    w: f32,
    i: usize,
    j: usize,
    k: usize,
    smooth: &[f32],
    up: &[f32],
    down: &[f32],
) -> f32 {
    let mut low = 0.0f32;
    for r in 0..RANK {
        let scaled_down = down[r * k + j] * smooth[j];
        let term = up[i * RANK + r] * scaled_down;
        low += term;
    }
    w * smooth[j] - low
}
fn residual_maximum(
    weight: &[f32],
    n: usize,
    k: usize,
    smooth: &[f32],
    up: &[f32],
    down: &[f32],
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<f32> {
    let mut maximum = 0.0f32;
    for i in 0..n {
        check()?;
        for j in 0..k {
            let x = residual_value(weight[i * k + j], i, j, k, smooth, up, down);
            if !x.is_finite() {
                return Err(fail("nonfinite residual"));
            }
            maximum = maximum.max(x.abs());
        }
    }
    Ok(maximum)
}
fn fake_block(block: &mut [f32], global: f32) {
    let amax = block.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    let scale = crate::format::decode::elem::e4m3(elem::e4m3(
        (amax / (6.0 * global)).clamp(2f32.powi(-9), 448.0),
    ));
    let unit = scale * global;
    for x in block {
        let code = if *x == 0.0 { 0 } else { elem::e2m1(*x / unit) };
        *x = (crate::format::decode::elem::e2m1(code) * scale) * global;
    }
}
fn input_quantize(
    rows: &[f32],
    transform: impl Fn(f32, usize) -> f32,
    global: f32,
    xq: &mut [f32],
    k: usize,
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    for (i, (src, dst)) in rows.chunks_exact(k).zip(xq.chunks_exact_mut(k)).enumerate() {
        let _ = i;
        check()?;
        for j in 0..k {
            dst[j] = rounded(transform(src[j], j))?;
        }
        for block in dst.chunks_exact_mut(16) {
            fake_block(block, global);
        }
    }
    finite(xq, "quantized calibration input")
}

// None is the plain NVFP4 baseline. Some applies the current BF16 low-rank
// factors and current canonical smoothing. `packed` selects the final residual
// bytes; otherwise the existing fast NVFP4 rounding rule scores the candidate.
fn score(
    work: &mut Work,
    weight: &[f32],
    calibration: &[f32],
    n: usize,
    k: usize,
    samples: usize,
    fitted: bool,
    global: f32,
    packed: Option<(&[u8], &[u8])>,
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<f64> {
    work.errors.fill(0.0);
    if fitted {
        product(
            row_mut(&mut work.projected, samples, RANK),
            row(calibration, samples, k),
            row(&work.down, RANK, k).transpose(),
            check,
        )?;
        for values in work.projected.chunks_mut(4096) {
            check()?;
            for x in values {
                *x = rounded(*x)?;
            }
        }
    }
    for first in (0..n).step_by(TILE) {
        check()?;
        let count = (n - first).min(TILE);
        let tile = &mut work.residual_tile[..count * k];
        for i in 0..count {
            for j in 0..k {
                tile[i * k + j] = if let Some((codes, scales)) = packed {
                    let code = codes[(first + i) * k / 2 + j / 2] >> ((j % 2) * 4) & 15;
                    let scale =
                        crate::format::decode::elem::e4m3(scales[(first + i) * (k / 16) + j / 16]);
                    (crate::format::decode::elem::e2m1(code) * scale) * global
                } else if fitted {
                    residual_value(
                        weight[(first + i) * k + j],
                        first + i,
                        j,
                        k,
                        &work.smooth,
                        &work.up,
                        &work.down,
                    )
                } else {
                    weight[(first + i) * k + j]
                };
            }
            if packed.is_none() {
                for block in tile[i * k..(i + 1) * k].chunks_exact_mut(16) {
                    fake_block(block, global);
                }
            }
        }
        finite(tile, "decoded residual tile")?;
        product(
            row_mut(&mut work.prediction[..samples * count], samples, count),
            row(&work.xq, samples, k),
            row(tile, count, k).transpose(),
            check,
        )?;
        if fitted {
            product(
                row_mut(&mut work.branch[..samples * count], samples, count),
                row(&work.projected, samples, RANK),
                row(&work.up[first * RANK..(first + count) * RANK], count, RANK).transpose(),
                check,
            )?;
        }
        for s in 0..samples {
            for i in 0..count {
                let y = work.prediction[s * count + i]
                    + if fitted {
                        work.branch[s * count + i]
                    } else {
                        0.0
                    };
                if !y.is_finite() {
                    return Err(fail("nonfinite calibration prediction"));
                }
                let difference = f64::from(y - work.reference[s * n + first + i]);
                work.errors[s] += difference * difference;
            }
        }
    }
    let mut mean = 0.0f64;
    for s in 0..samples {
        mean += work.errors[s].sqrt() / work.norms[s].max(1e-30);
    }
    let mean = mean / samples as f64;
    if !mean.is_finite() {
        return Err(fail("nonfinite calibration error"));
    }
    Ok(mean)
}

fn part(role: &str, dtype: Dtype, shape: &[u64], bytes: Vec<u8>) -> Result<OwnedPart> {
    let mut name = String::new();
    name.try_reserve_exact(role.len())
        .map_err(|_| fail("cannot allocate part role"))?;
    name.push_str(role);
    let mut dims = zeros(shape.len())?;
    dims.copy_from_slice(shape);
    Ok(OwnedPart {
        role: name,
        dtype,
        shape: dims,
        bytes,
    })
}
fn scalar(x: f32) -> Result<Vec<u8>> {
    let mut bytes = zeros(4)?;
    bytes.copy_from_slice(&x.to_le_bytes());
    Ok(bytes)
}
fn float_bytes(values: &[f32], bf16: bool) -> Result<Vec<u8>> {
    let width = if bf16 { 2 } else { 4 };
    let mut bytes = zeros(mul(values.len(), width)?)?;
    for (x, b) in values.iter().zip(bytes.chunks_exact_mut(width)) {
        if bf16 {
            b.copy_from_slice(&elem::bf16(*x).to_le_bytes());
        } else {
            b.copy_from_slice(&x.to_le_bytes());
        }
    }
    Ok(bytes)
}

/// Fit a complete tensor, including alpha selection and served calibration score.
/// A single faer QR/SVD call is not internally cancellable; every surrounding
/// pass, input/output tile and alpha observes the caller's native control.
#[allow(clippy::too_many_arguments)]
pub fn svdq_fit(
    weight: &[f32],
    xmax: &[f32],
    calibration: &[f32],
    rows: usize,
    cols: usize,
    samples: usize,
    seed: u64,
    max_buffer_bytes: usize,
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<SvdqFitted> {
    check()?;
    let q = geometry(rows, cols, samples)?;
    let required = svdq_fit_workspace(rows, cols, samples)?;
    if required > max_buffer_bytes {
        return Err(fail(format!(
            "requires {required} working bytes; grant is {max_buffer_bytes}"
        )));
    }
    if weight.len() != rows * cols || xmax.len() != cols || calibration.len() != samples * cols {
        return Err(fail(
            "weight/xmax/calibration slice lengths do not match [N,K]/[K]/[S,K]",
        ));
    }
    for (values, name) in [
        (weight, "weight"),
        (xmax, "channel maxima"),
        (calibration, "calibration rows"),
    ] {
        for window in values.chunks(4096) {
            check()?;
            finite(window, name)?;
        }
    }
    if xmax.iter().any(|x| *x < 0.0) {
        return Err(fail("channel maxima must be nonnegative"));
    }
    let mut work = Work::new(rows, cols, samples, q)?;
    product(
        row_mut(&mut work.reference, samples, rows),
        row(calibration, samples, cols),
        row(weight, rows, cols).transpose(),
        check,
    )?;
    finite(&work.reference, "reference outputs")?;
    for s in 0..samples {
        work.norms[s] = work.reference[s * rows..(s + 1) * rows]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            .sqrt();
    }
    let mut weight_max = 0.0f32;
    for values in weight.chunks_exact(cols) {
        check()?;
        for (maximum, value) in work.wmax.iter_mut().zip(values) {
            *maximum = maximum.max(value.abs());
            weight_max = weight_max.max(value.abs());
        }
    }
    let input_max = calibration.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    input_quantize(
        calibration,
        |x, _| x,
        (input_max / LEVELS).max(1e-30),
        &mut work.xq,
        cols,
        check,
    )?;
    let plain = score(
        &mut work,
        weight,
        calibration,
        rows,
        cols,
        samples,
        false,
        (weight_max / LEVELS).max(1e-30),
        None,
        check,
    )?;
    let mut best_error = f64::INFINITY;
    let mut best_alpha = 0.0f32;
    for alpha in ALPHAS {
        check()?;
        let mut mean = 0.0f32;
        for j in 0..cols {
            let s = if alpha == 0.0 {
                1.0
            } else {
                (xmax[j].max(1e-5).powf(alpha) / work.wmax[j].max(1e-5).powf(1.0 - alpha)).max(1e-5)
            };
            work.smooth[j] = s;
            mean += s;
        }
        mean /= cols as f32;
        if !mean.is_finite() || mean <= 0.0 {
            return Err(fail("unrepresentable smoothing mean"));
        }
        for s in &mut work.smooth {
            let pre = 1.0 / (*s / mean);
            *s = 1.0 / pre;
            if !pre.is_finite() || pre <= 0.0 || !s.is_finite() || *s <= 0.0 {
                return Err(fail("unrepresentable smoothing scale"));
            }
        }
        for i in 0..rows {
            check()?;
            for j in 0..cols {
                work.a[i * cols + j] = weight[i * cols + j] * work.smooth[j];
            }
        }
        finite(&work.a, "smoothed weight")?;
        work.factors(rows, cols, q, seed, check)?;
        let maximum = residual_maximum(
            weight,
            rows,
            cols,
            &work.smooth,
            &work.up,
            &work.down,
            check,
        )?;
        let gx = (xmax
            .iter()
            .zip(&work.smooth)
            .fold(0.0f32, |m, (x, s)| m.max(x.max(1e-5) / s))
            * 2.0
            / LEVELS)
            .max(1e-30);
        if !gx.is_finite() || gx <= 0.0 {
            return Err(fail("unrepresentable activation scale"));
        }
        input_quantize(
            calibration,
            |x, j| x / work.smooth[j],
            gx,
            &mut work.xq,
            cols,
            check,
        )?;
        let error = score(
            &mut work,
            weight,
            calibration,
            rows,
            cols,
            samples,
            true,
            (maximum / LEVELS).max(1e-30),
            None,
            check,
        )?;
        if error < best_error {
            best_error = error;
            best_alpha = alpha;
            work.best_up.copy_from_slice(&work.up);
            work.best_down.copy_from_slice(&work.down);
            for (p, s) in work.pre.iter_mut().zip(&work.smooth) {
                *p = 1.0 / s;
            }
        }
    }
    check()?;
    work.up.copy_from_slice(&work.best_up);
    work.down.copy_from_slice(&work.best_down);
    for (s, p) in work.smooth.iter_mut().zip(&work.pre) {
        *s = 1.0 / p;
    }
    let maximum = residual_maximum(
        weight,
        rows,
        cols,
        &work.smooth,
        &work.up,
        &work.down,
        check,
    )?;
    let global = nvfp4::min_sse_global(f64::from(maximum))?;
    let mut codes = zeros(rows * cols / 2)?;
    let mut scales = zeros(rows * cols / 16)?;
    let mut residual = QuantizationStats::default();
    let mut stats = QuantizationStats::default();
    for i in 0..rows {
        check()?;
        for j in (0..cols).step_by(16) {
            let block: [f32; 16] = std::array::from_fn(|b| {
                residual_value(
                    weight[i * cols + j + b],
                    i,
                    j + b,
                    cols,
                    &work.smooth,
                    &work.up,
                    &work.down,
                )
            });
            let (scale, nibbles, saturated) = nvfp4::min_sse_block(&block, global);
            scales[i * (cols / 16) + j / 16] = scale;
            for b in 0..8 {
                codes[i * (cols / 2) + j / 2 + b] = nibbles[2 * b] | (nibbles[2 * b + 1] << 4);
            }
            residual.saturated += saturated;
            stats.saturated += saturated;
            let scale = crate::format::decode::elem::e4m3(scale);
            for b in 0..16 {
                let code = crate::format::decode::elem::e2m1(nibbles[b]);
                let decoded = (code * scale) * global;
                residual.add(block[b], f64::from(decoded));
                // Match decode::plan::Post::LowRank: every stored-value
                // operation rounds in F32; only the error accumulation is F64.
                let mut logical = decoded * work.pre[j + b];
                for r in 0..RANK {
                    logical += work.up[i * RANK + r] * work.down[r * cols + j + b];
                }
                stats.add(weight[i * cols + j + b], f64::from(logical));
            }
        }
    }
    let gx = (xmax
        .iter()
        .zip(&work.smooth)
        .fold(0.0f32, |m, (x, s)| m.max(x.max(1e-5) / s))
        * 2.0
        / LEVELS)
        .max(1e-30);
    input_quantize(
        calibration,
        |x, j| x * work.pre[j],
        gx,
        &mut work.xq,
        cols,
        check,
    )?;
    let served = score(
        &mut work,
        weight,
        calibration,
        rows,
        cols,
        samples,
        true,
        global,
        Some((&codes, &scales)),
        check,
    )?;
    check()?;
    let mut parts = Vec::new();
    parts
        .try_reserve_exact(7)
        .map_err(|_| fail("cannot allocate output parts"))?;
    parts.push(part(
        "weight",
        Dtype::F4,
        &[rows as u64, cols as u64],
        codes,
    )?);
    parts.push(part(
        "scale",
        Dtype::F8_E4M3,
        &[rows as u64, (cols / 16) as u64],
        scales,
    )?);
    parts.push(part("scale_2", Dtype::F32, &[], scalar(global)?)?);
    parts.push(part("input_scale", Dtype::F32, &[], scalar(gx)?)?);
    parts.push(part(
        "pre_quant_scale",
        Dtype::F32,
        &[cols as u64],
        float_bytes(&work.pre, false)?,
    )?);
    parts.push(part(
        "lowrank_down",
        Dtype::BF16,
        &[RANK as u64, cols as u64],
        float_bytes(&work.down, true)?,
    )?);
    parts.push(part(
        "lowrank_up",
        Dtype::BF16,
        &[rows as u64, RANK as u64],
        float_bytes(&work.up, true)?,
    )?);
    check()?;
    Ok(SvdqFitted {
        parts,
        stats,
        residual,
        alpha: best_alpha,
        calibration_error: served,
        plain_nvfp4_error: plain,
        required_buffer_bytes: required,
    })
}
