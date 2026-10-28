//! The complete, bounded CPU producer for `svdq.nvfp4`.
//!
//! The caller owns immutable row-major F32 inputs. Their bytes count against this
//! operation's grant. File read/decode staging must be charged separately by Writer.
//! Numerical matrices and encoded parts are fallibly allocated once and reused. The
//! patched faer dependency uses one-column Householder blocks, and q <= 48 stays in
//! its bidiagonal QR base case; no unreported GEMM packing cache is entered.
//!
//! The work is spread over one pool of the machine's cores (`pool`) without changing a
//! bit: every product is the same sequence of 16-cubed faer calls per output tile, each
//! tile on one thread; row-wise passes are independent per row; maxima are exact in any
//! order; and every floating-point sum keeps its sequential order (a calibration row's
//! error over the outputs, the weight's error statistics over its elements).

use faer::diag::DiagMut;
use faer::dyn_stack::{MemBuffer, MemStack, StackReq};
use faer::linalg::{householder, qr::no_pivoting::factor, svd};
use faer::prelude::ReborrowMut;
use faer::{Accum, Conj, MatMut, MatRef, Par};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use rand_distr::{Distribution, StandardNormal};
use rayon::prelude::*;
use std::sync::OnceLock;

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
    /// The workers the fit ran on (`threads`).
    pub workers: usize,
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
    if values.par_chunks(1 << 16).any(|c| c.iter().any(|x| !x.is_finite())) {
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
/// The fit's workers: one per core the process may use, made once.
fn pool() -> Result<&'static rayon::ThreadPool> {
    static POOL: OnceLock<std::result::Result<rayon::ThreadPool, String>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads())
            .thread_name(|i| format!("tensorfs-svdq-{i}"))
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| fail(format!("cannot start the fit's workers: {e}")))
}
fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
type Check<'c> = &'c (dyn Fn() -> Result<()> + Sync);

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
fn padded(rows: usize, cols: usize) -> Result<usize> {
    mul(rows.div_ceil(TILE) * TILE, cols.div_ceil(TILE) * TILE)
}

pub fn svdq_fit_workspace(rows: usize, cols: usize, samples: usize) -> Result<usize> {
    add(shared_workspace(rows, cols, samples)?, alpha_workspace(rows, cols, samples)?)
}

/// What a fit holds once, whatever its alphas in flight: the immutable F32 inputs; the reference
/// outputs and norms; the transposed weight (while the reference is made) or the final logical
/// weight; channel maxima, the best factors and pre; a parallel product's tiles; each worker's
/// residual tile, its transpose, branch, residual row and product panel; the seven parts.
fn shared_workspace(rows: usize, cols: usize, samples: usize) -> Result<usize> {
    let q = geometry(rows, cols, samples)?;
    let nk = mul(rows, cols)?;
    let worker = add(
        add(mul(8 * TILE, cols)?, mul(samples, 4 * TILE)?)?,
        add(cols, add(mul(cols, 256)?, TILE * 256)?)?,
    )?;
    let counts = [
        nk,
        mul(samples, cols)?,
        cols,
        mul(samples, rows)?,
        nk,
        cols,
        mul(rows, RANK)?,
        mul(RANK, cols)?,
        cols,
        padded(samples, rows)?.max(padded(rows, q)?).max(padded(cols, q)?),
        mul(threads(), worker)?,
    ];
    let mut bytes = 64 * 1024; // bounded stack/Vec/part/shape/string metadata and RNG state
    for count in counts {
        bytes = add(bytes, mul(count, 4)?)?;
    }
    bytes = add(bytes, mul(samples, 8)?)?;
    for count in [nk / 2, nk / 16, 8, mul(cols, 4)?, mul(rows, 2 * RANK)?, mul(cols, 2 * RANK)?] {
        bytes = add(bytes, count)?;
    }
    Ok(bytes)
}

/// One alpha in flight (`Work`): the smoothed weight and its transpose; the sketch, basis and
/// range (column- and row-major), V, U, singular values and Householder coefficients; the
/// factors, down scaled and transposed; the smoothing; quantized activations, projected rows,
/// a score's predicted outputs and errors; faer's scratch.
fn alpha_workspace(rows: usize, cols: usize, samples: usize) -> Result<usize> {
    let q = geometry(rows, cols, samples)?;
    let (nk, nq, kq) = (mul(rows, cols)?, mul(rows, q)?, mul(cols, q)?);
    let counts = [
        nk,
        nk,
        nq,
        nq,
        nq,
        nq,
        kq,
        kq,
        kq,
        mul(q, q)?,
        q,
        q,
        mul(rows, RANK)?,
        mul(RANK, cols)?,
        mul(RANK, cols)?,
        mul(RANK, cols)?,
        cols,
        mul(samples, cols)?,
        mul(samples, RANK)?,
        mul(samples, rows)?,
    ];
    let mut bytes = 0usize;
    for count in counts {
        bytes = add(bytes, mul(count, 4)?)?;
    }
    bytes = add(bytes, mul(samples, 8)?)?;
    add(bytes, scratch(rows, cols, q)?.unaligned_bytes_required())
}

fn col(values: &[f32], rows: usize, cols: usize) -> MatRef<'_, f32> {
    MatRef::from_column_major_slice(values, rows, cols)
}
fn col_mut(values: &mut [f32], rows: usize, cols: usize) -> MatMut<'_, f32> {
    MatMut::from_column_major_slice_mut(values, rows, cols)
}

/// A product's operand: a strided matrix over a slice, as faer reads it (shared by workers).
#[derive(Clone, Copy)]
struct View<'a> {
    data: &'a [f32],
    offset: usize,
    rows: usize,
    cols: usize,
    rs: usize,
    cs: usize,
}
impl<'a> View<'a> {
    fn row(data: &'a [f32], rows: usize, cols: usize) -> Self {
        Self { data, offset: 0, rows, cols, rs: cols, cs: 1 }
    }
    fn col(data: &'a [f32], rows: usize, cols: usize) -> Self {
        Self { data, offset: 0, rows, cols, rs: 1, cs: rows }
    }
    fn transpose(self) -> Self {
        Self { rows: self.cols, cols: self.rows, rs: self.cs, cs: self.rs, ..self }
    }
    fn sub(self, i: usize, j: usize, rows: usize, cols: usize) -> Self {
        Self { offset: self.offset + i * self.rs + j * self.cs, rows, cols, ..self }
    }
    fn mat(self) -> MatRef<'a, f32> {
        let data = &self.data[self.offset..];
        if self.cs == 1 {
            MatRef::from_row_major_slice_with_stride(data, self.rows, self.cols, self.rs)
        } else {
            MatRef::from_column_major_slice_with_stride(data, self.rows, self.cols, self.cs)
        }
    }
}

/// A product's destination: a row- or column-major slice.
struct Out<'a> {
    data: &'a mut [f32],
    rows: usize,
    cols: usize,
    row_major: bool,
}
impl<'a> Out<'a> {
    fn row(data: &'a mut [f32], rows: usize, cols: usize) -> Self {
        Self { data, rows, cols, row_major: true }
    }
    fn col(data: &'a mut [f32], rows: usize, cols: usize) -> Self {
        Self { data, rows, cols, row_major: false }
    }
}

/// One output tile `[i, i + m) x [j, j + n)` of `lhs * rhs`: <= 16^3 operations a call
/// selects faer's allocation-free nano-GEMM path even on machines whose regular GEMM
/// backend keeps a large thread-local packing cache. Depth accumulates in 16s, in order.
fn product_tile(mut dst: MatMut<'_, f32>, lhs: View<'_>, rhs: View<'_>, i: usize, j: usize) {
    let (m, n) = (dst.nrows(), dst.ncols());
    for k in (0..lhs.cols).step_by(TILE) {
        let depth = (lhs.cols - k).min(TILE);
        faer::linalg::matmul::matmul(
            dst.rb_mut(),
            if k == 0 { Accum::Replace } else { Accum::Add },
            lhs.sub(i, k, m, depth).mat(),
            rhs.sub(k, j, depth, n).mat(),
            1.0,
            Par::Seq,
        );
    }
}

/// `dst = lhs * rhs`, its tiles on the pool's workers. A worker writes its tile in a
/// buffer laid out as `dst` is (row- or column-major, so faer takes the same kernel), and
/// the tiles are then copied into place.
fn product(dst: Out<'_>, lhs: View<'_>, rhs: View<'_>, check: Check) -> Result<()> {
    assert!(dst.rows == lhs.rows && dst.cols == rhs.cols && lhs.cols == rhs.rows);
    let (rows, cols, row_major) = (dst.rows, dst.cols, dst.row_major);
    let across = cols.div_ceil(TILE);
    let tiles = (0..rows.div_ceil(TILE) * across)
        .into_par_iter()
        .map(|t| {
            check()?;
            let (i, j) = (t / across * TILE, t % across * TILE);
            let (m, n) = ((rows - i).min(TILE), (cols - j).min(TILE));
            let mut tile = [0.0f32; TILE * TILE];
            // (faer's from_row_major_slice_with_stride_mut lays its slice out column-major.)
            let out = if row_major {
                MatMut::from_column_major_slice_with_stride_mut(&mut tile, n, m, TILE).transpose_mut()
            } else {
                MatMut::from_column_major_slice_with_stride_mut(&mut tile, m, n, TILE)
            };
            product_tile(out, lhs, rhs, i, j);
            Ok(tile)
        })
        .collect::<Result<Vec<_>>>()?;
    for (t, tile) in tiles.iter().enumerate() {
        let (i, j) = (t / across * TILE, t % across * TILE);
        let (m, n) = ((rows - i).min(TILE), (cols - j).min(TILE));
        for r in 0..m {
            for c in 0..n {
                if row_major {
                    dst.data[(i + r) * cols + j + c] = tile[r * TILE + c];
                } else {
                    dst.data[(i + r) + (j + c) * rows] = tile[r + c * TILE];
                }
            }
        }
    }
    Ok(())
}

/// Whether faer takes its nano-GEMM path for every 16-cubed call of an `m x n` product of
/// depth `kk` (`product_tile`): a one-row or one-column tile, or a depth-one block, takes its
/// matrix-vector or rank-update paths instead, which `exact` does not replicate.
fn nano(m: usize, n: usize, kk: usize) -> bool {
    m % TILE != 1 && n % TILE != 1 && kk % TILE != 1
}

/// `out[m, n] = a[m, kk] * b[kk, n]`, all row-major, each element summed exactly as faer's
/// 16-cubed nano-GEMM calls sum it (`product_tile`, when `nano`): per 16-deep block a fused
/// multiply-add chain from +0 in depth order, the blocks then added in order. Vectorized
/// across columns; the order within an element is unchanged.
struct Exact<'a> {
    a: &'a [f32],
    b: &'a [f32],
    out: &'a mut [f32],
    kk: usize,
    n: usize,
}
impl pulp::WithSimd for Exact<'_> {
    type Output = ();
    #[inline(always)]
    fn with_simd<S: pulp::Simd>(self, simd: S) {
        let Exact { a, b, out, kk, n } = self;
        // Rows outer, so a group's rows of `a` stay in cache across `b`'s columns.
        let m = out.len() / n;
        let width = 2 * S::F32_LANES;
        let mut r = 0;
        while r < m {
            let rows = if r + 4 <= m { 4 } else { 1 };
            let (a, out) = (&a[r * kk..(r + rows) * kk], &mut out[r * n..(r + rows) * n]);
            for c in (0..n).step_by(width) {
                let w = (n - c).min(width);
                if rows == 4 {
                    panel::<S, 4>(simd, a, b, out, kk, n, c, w);
                } else {
                    panel::<S, 1>(simd, a, b, out, kk, n, c, w);
                }
            }
            r += rows;
        }
    }
}

/// `ROWS` rows of `exact`, columns `[c, c + w)` (`w` up to two vectors).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn panel<S: pulp::Simd, const ROWS: usize>(
    simd: S,
    a: &[f32],
    b: &[f32],
    out: &mut [f32],
    kk: usize,
    n: usize,
    c: usize,
    w: usize,
) {
    let lanes = S::F32_LANES;
    let zero = simd.splat_f32s(0.0);
    let mut sum = [[zero; 2]; ROWS];
    for block in (0..kk).step_by(TILE) {
        let mut acc = [[zero; 2]; ROWS];
        for d in block..(block + TILE).min(kk) {
            let row = &b[d * n + c..d * n + c + w];
            let (b0, b1) = if w == 2 * lanes {
                let (v, _) = S::as_simd_f32s(row);
                (v[0], v[1])
            } else if w > lanes {
                (simd.partial_load_f32s(&row[..lanes]), simd.partial_load_f32s(&row[lanes..]))
            } else {
                (simd.partial_load_f32s(row), zero)
            };
            for i in 0..ROWS {
                let x = simd.splat_f32s(a[i * kk + d]);
                acc[i][0] = simd.mul_add_f32s(x, b0, acc[i][0]);
                acc[i][1] = simd.mul_add_f32s(x, b1, acc[i][1]);
            }
        }
        for i in 0..ROWS {
            sum[i][0] = simd.add_f32s(acc[i][0], sum[i][0]);
            sum[i][1] = simd.add_f32s(acc[i][1], sum[i][1]);
        }
    }
    for i in 0..ROWS {
        let row = &mut out[i * n + c..i * n + c + w];
        if w > lanes {
            let (head, tail) = row.split_at_mut(lanes);
            simd.partial_store_f32s(head, sum[i][0]);
            simd.partial_store_f32s(tail, sum[i][1]);
        } else {
            simd.partial_store_f32s(row, sum[i][0]);
        }
    }
}

/// `exact` on this thread.
fn exact(out: &mut [f32], a: &[f32], b: &[f32], kk: usize, n: usize) {
    pulp::Arch::new().dispatch(Exact { a, b, out, kk, n });
}

/// `exact`, its 16-row bands of `out` on the pool's workers; within a band, columns go in
/// blocks of 256 so a block of `b` stays in cache.
fn exact_par(out: &mut [f32], a: &[f32], b: &[f32], kk: usize, n: usize, check: Check) -> Result<()> {
    const COLUMNS: usize = 256;
    out.par_chunks_mut(TILE * n).enumerate().try_for_each(|(band, out)| {
        check()?;
        let rows = out.len() / n;
        let a = &a[band * TILE * kk..(band * TILE + rows) * kk];
        if n <= COLUMNS {
            exact(out, a, b, kk, n);
            return Ok(());
        }
        let mut block = [0.0f32; TILE * COLUMNS];
        let mut panel = Vec::new();
        for c in (0..n).step_by(COLUMNS) {
            let w = (n - c).min(COLUMNS);
            panel.clear();
            for d in 0..kk {
                panel.extend_from_slice(&b[d * n + c..d * n + c + w]);
            }
            exact(&mut block[..rows * w], a, &panel, kk, w);
            for i in 0..rows {
                out[i * n + c..i * n + c + w].copy_from_slice(&block[i * w..(i + 1) * w]);
            }
        }
        Ok(())
    })
}

/// `src [rows, cols]` row-major, transposed into `dst [cols, rows]` row-major.
fn transposed(dst: &mut [f32], src: &[f32], rows: usize, cols: usize) {
    dst.par_chunks_mut(rows).enumerate().for_each(|(j, row)| {
        for (i, x) in row.iter_mut().enumerate() {
            *x = src[i * cols + j];
        }
    });
}

/// `dst = lhs * rhs` on this thread (a worker's own small product).
fn product_seq(mut dst: MatMut<'_, f32>, lhs: View<'_>, rhs: View<'_>) {
    for i in (0..dst.nrows()).step_by(TILE) {
        let m = (dst.nrows() - i).min(TILE);
        for j in (0..dst.ncols()).step_by(TILE) {
            let n = (dst.ncols() - j).min(TILE);
            product_tile(dst.rb_mut().submatrix_mut(i, j, m, n), lhs, rhs, i, j);
        }
    }
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
    smooth: Vec<f32>,
    xq: Vec<f32>,
    projected: Vec<f32>,
    /// `a` transposed, [K, N]; `z`, `basis` and `y` row-major; `down`
    /// times the smoothing, [R, K]; `down` transposed, [K, R].
    at: Vec<f32>,
    zr: Vec<f32>,
    br: Vec<f32>,
    yr: Vec<f32>,
    scaled: Vec<f32>,
    downt: Vec<f32>,
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
            smooth: zeros(k)?,
            xq: zeros(s * k)?,
            projected: zeros(s * RANK)?,
            at: zeros(n * k)?,
            zr: zeros(k * q)?,
            br: zeros(n * q)?,
            yr: zeros(n * q)?,
            scaled: zeros(RANK * k)?,
            downt: zeros(k * RANK)?,
            errors: zeros(s)?,
            scratch: MemBuffer::try_new(scratch(n, k, q)?)
                .map_err(|_| fail("cannot allocate faer scratch"))?,
        })
    }
    fn orthogonalize(
        &mut self,
        n: usize,
        q: usize,
        check: Check,
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
    /// y = a z: [n, q], depth k.
    fn range(&mut self, n: usize, k: usize, q: usize, check: Check) -> Result<()> {
        if !nano(n, q, k) {
            return product(
                Out::col(&mut self.y, n, q),
                View::row(&self.a, n, k),
                View::col(&self.z, k, q),
                check,
            );
        }
        transposed(&mut self.zr, &self.z, q, k);
        exact_par(&mut self.yr, &self.a, &self.zr, k, q, check)?;
        transposed(&mut self.y, &self.yr, n, q);
        Ok(())
    }
    /// z = a^T basis: [k, q], depth n.
    fn project(&mut self, n: usize, k: usize, q: usize, check: Check) -> Result<()> {
        if !nano(k, q, n) {
            return product(
                Out::col(&mut self.z, k, q),
                View::row(&self.a, n, k).transpose(),
                View::col(&self.basis, n, q),
                check,
            );
        }
        transposed(&mut self.br, &self.basis, q, n);
        exact_par(&mut self.zr, &self.at, &self.br, n, q, check)?;
        transposed(&mut self.z, &self.zr, k, q);
        Ok(())
    }
    fn factors(
        &mut self,
        n: usize,
        k: usize,
        q: usize,
        seed: u64,
        check: Check,
    ) -> Result<()> {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        for chunk in self.z.chunks_mut(4096) {
            check()?;
            for x in chunk {
                *x = StandardNormal.sample(&mut rng);
            }
        }
        if nano(k, q, n) {
            transposed(&mut self.at, &self.a, n, k);
        }
        self.range(n, k, q, check)?;
        for _ in 0..4 {
            self.orthogonalize(n, q, check)?;
            self.project(n, k, q, check)?;
            finite(&self.z, "subspace projection")?;
            self.range(n, k, q, check)?;
        }
        self.orthogonalize(n, q, check)?;
        self.project(n, k, q, check)?;
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
        if nano(n, RANK, q) {
            // u's first RANK columns, row-major [q, RANK]; the basis row-major [n, q].
            let ur: Vec<f32> = (0..q * RANK).map(|e| self.u[e / RANK + (e % RANK) * q]).collect();
            transposed(&mut self.br, &self.basis, q, n);
            exact_par(&mut self.up, &self.br, &ur, q, RANK, check)?;
        } else {
            product(
                Out::row(&mut self.up, n, RANK),
                View::col(&self.basis, n, q),
                View::col(&self.u, q, q).sub(0, 0, q, RANK),
                check,
            )?;
        }
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

/// `down` times the smoothing, `[R, K]`: `scaled[r, j] = down[r, j] * smooth[j]`.
fn scale_down(scaled: &mut [f32], down: &[f32], smooth: &[f32], k: usize) {
    for (scaled, down) in scaled.chunks_exact_mut(k).zip(down.chunks_exact(k)) {
        for j in 0..k {
            scaled[j] = down[j] * smooth[j];
        }
    }
}
/// Row `i` of the residual into `out` (`[K]`), in this explicit F32 operation order per
/// element: `low = sum_r (up[i, r] * scaled[r, j])` from 0 in rank order, then
/// `w * smooth[j] - low`. Columns go together; each element's order is unchanged.
fn residual_row(out: &mut [f32], weight: &[f32], i: usize, smooth: &[f32], up: &[f32], scaled: &[f32]) {
    let k = out.len();
    out.fill(0.0);
    for r in 0..RANK {
        let u = up[i * RANK + r];
        for (low, s) in out.iter_mut().zip(&scaled[r * k..(r + 1) * k]) {
            *low += u * *s;
        }
    }
    for ((x, w), s) in out.iter_mut().zip(&weight[i * k..(i + 1) * k]).zip(smooth) {
        *x = *w * *s - *x;
    }
}
fn residual_maximum(
    weight: &[f32],
    n: usize,
    k: usize,
    smooth: &[f32],
    up: &[f32],
    scaled: &[f32],
    check: Check,
) -> Result<f32> {
    // A maximum is exact in any order: rows on the workers.
    (0..n)
        .into_par_iter()
        .map_init(Vec::new, |row, i| {
            check()?;
            row.resize(k, 0.0);
            residual_row(row, weight, i, smooth, up, scaled);
            let mut maximum = 0.0f32;
            for x in row.iter() {
                if !x.is_finite() {
                    return Err(fail("nonfinite residual"));
                }
                maximum = maximum.max(x.abs());
            }
            Ok(maximum)
        })
        .try_reduce(|| 0.0f32, |a, b| Ok(a.max(b)))
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
    transform: impl Fn(f32, usize) -> f32 + Sync,
    global: f32,
    xq: &mut [f32],
    k: usize,
    check: Check,
) -> Result<()> {
    rows.par_chunks_exact(k).zip(xq.par_chunks_exact_mut(k)).try_for_each(|(src, dst)| {
        check()?;
        for j in 0..k {
            dst[j] = rounded(transform(src[j], j))?;
        }
        for block in dst.chunks_exact_mut(16) {
            fake_block(block, global);
        }
        Ok(())
    })?;
    finite(xq, "quantized calibration input")
}

// None is the plain NVFP4 baseline. Some applies the current BF16 low-rank
// factors and current canonical smoothing. `packed` selects the final residual
// bytes; otherwise the existing fast NVFP4 rounding rule scores the candidate.
// Each 16-row residual tile is one worker's; each calibration row then sums its
// squared errors over the outputs in order.
#[allow(clippy::too_many_arguments)]
fn score(
    work: &mut Work,
    weight: &[f32],
    calibration: &[f32],
    reference: &Reference,
    n: usize,
    k: usize,
    samples: usize,
    fitted: bool,
    global: f32,
    packed: Option<(&[u8], &[u8])>,
    check: Check,
) -> Result<f64> {
    if fitted {
        if nano(samples, RANK, k) {
            transposed(&mut work.downt, &work.down, RANK, k);
            exact_par(&mut work.projected, calibration, &work.downt, k, RANK, check)?;
        } else {
            product(
                Out::row(&mut work.projected, samples, RANK),
                View::row(calibration, samples, k),
                View::row(&work.down, RANK, k).transpose(),
                check,
            )?;
        }
        for values in work.projected.chunks_mut(4096) {
            check()?;
            for x in values {
                *x = rounded(*x)?;
            }
        }
    }
    let (smooth, up, scaled) = (&work.smooth, &work.up, &work.scaled);
    let (xq, projected) = (&work.xq, &work.projected);
    let grow = |buffer: &mut Vec<f32>, len: usize| -> Result<()> {
        if buffer.len() < len {
            buffer
                .try_reserve_exact(len - buffer.len())
                .map_err(|_| fail(format!("cannot allocate {len} workspace elements")))?;
            buffer.resize(len, 0.0);
        }
        Ok(())
    };
    // Each group of output rows' predicted outputs, [S, count] row-major. With the 16-cubed
    // calls all nano-GEMM (`nano`), a worker takes four 16-row tiles at a time; otherwise one,
    // through faer itself.
    let grouped = nano(samples, n, k);
    let group = if grouped { 4 * TILE } else { TILE };
    let predicted = (0..n.div_ceil(group))
        .into_par_iter()
        .map_init(
            || (Vec::new(), Vec::new(), Vec::new()),
            |(tile, branch, transpose), t| -> Result<Vec<f32>> {
                check()?;
                let first = t * group;
                let count = (n - first).min(group);
                grow(tile, count * k)?;
                grow(branch, samples * count)?;
                grow(transpose, count * k.max(RANK))?;
                let tile = &mut tile[..count * k];
                for i in 0..count {
                    let row = &mut tile[i * k..(i + 1) * k];
                    if let Some((codes, scales)) = packed {
                        for j in 0..k {
                            let code = codes[(first + i) * k / 2 + j / 2] >> ((j % 2) * 4) & 15;
                            let scale = crate::format::decode::elem::e4m3(
                                scales[(first + i) * (k / 16) + j / 16],
                            );
                            row[j] = (crate::format::decode::elem::e2m1(code) * scale) * global;
                        }
                    } else if fitted {
                        residual_row(row, weight, first + i, smooth, up, scaled);
                    } else {
                        row.copy_from_slice(&weight[(first + i) * k..(first + i + 1) * k]);
                    }
                    if packed.is_none() {
                        for block in tile[i * k..(i + 1) * k].chunks_exact_mut(16) {
                            fake_block(block, global);
                        }
                    }
                }
                finite(tile, "decoded residual tile")?;
                let mut prediction = zeros(samples * count)?;
                if grouped {
                    let tiled = &mut transpose[..count * k];
                    for (j, column) in tiled.chunks_exact_mut(count).enumerate() {
                        for (i, x) in column.iter_mut().enumerate() {
                            *x = tile[i * k + j];
                        }
                    }
                    exact(&mut prediction, xq, tiled, k, count);
                } else {
                    product_seq(
                        MatMut::from_row_major_slice_mut(&mut prediction, samples, count),
                        View::row(xq, samples, k),
                        View::row(tile, count, k).transpose(),
                    );
                }
                let branch = &mut branch[..samples * count];
                let up = &up[first * RANK..(first + count) * RANK];
                if fitted && grouped {
                    let tiled = &mut transpose[..count * RANK];
                    for (r, column) in tiled.chunks_exact_mut(count).enumerate() {
                        for (i, x) in column.iter_mut().enumerate() {
                            *x = up[i * RANK + r];
                        }
                    }
                    exact(branch, projected, tiled, RANK, count);
                } else if fitted {
                    product_seq(
                        MatMut::from_row_major_slice_mut(branch, samples, count),
                        View::row(projected, samples, RANK),
                        View::row(up, count, RANK).transpose(),
                    );
                }
                for (y, b) in prediction.iter_mut().zip(branch.iter()) {
                    *y += if fitted { *b } else { 0.0 };
                    if !y.is_finite() {
                        return Err(fail("nonfinite calibration prediction"));
                    }
                }
                Ok(prediction)
            },
        )
        .collect::<Result<Vec<_>>>()?;
    // Each calibration row sums its squared errors over the outputs in order.
    let (norms, reference) = (&reference.norms, &reference.outputs);
    work.errors.par_iter_mut().enumerate().for_each(|(s, error)| {
        let mut sum = 0.0f64;
        for (t, tile) in predicted.iter().enumerate() {
            let count = tile.len() / samples;
            for i in 0..count {
                let difference = f64::from(tile[s * count + i] - reference[s * n + t * group + i]);
                sum += difference * difference;
            }
        }
        *error = sum;
    });
    let mut mean = 0.0f64;
    for s in 0..samples {
        mean += work.errors[s].sqrt() / norms[s].max(1e-30);
    }
    let mean = mean / samples as f64;
    if !mean.is_finite() {
        return Err(fail("nonfinite calibration error"));
    }
    Ok(mean)
}

/// What every alpha reads: the calibration rows' outputs through the weight, and their norms.
struct Reference {
    outputs: Vec<f32>,
    norms: Vec<f64>,
}

/// One alpha's candidate in `work`: the smoothing, the low-rank factors of the smoothed weight,
/// and its calibration error. Alphas share nothing they write, so any number run at once.
#[allow(clippy::too_many_arguments)]
fn candidate(
    work: &mut Work,
    alpha: f32,
    weight: &[f32],
    xmax: &[f32],
    wmax: &[f32],
    calibration: &[f32],
    reference: &Reference,
    (rows, cols, samples, q, seed): (usize, usize, usize, usize, u64),
    check: Check,
) -> Result<f64> {
    check()?;
    let mut mean = 0.0f32;
    for j in 0..cols {
        let s = if alpha == 0.0 {
            1.0
        } else {
            (xmax[j].max(1e-5).powf(alpha) / wmax[j].max(1e-5).powf(1.0 - alpha)).max(1e-5)
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
    let smooth = &work.smooth;
    work.a.par_chunks_exact_mut(cols).zip(weight.par_chunks_exact(cols)).for_each(|(a, w)| {
        for j in 0..cols {
            a[j] = w[j] * smooth[j];
        }
    });
    finite(&work.a, "smoothed weight")?;
    work.factors(rows, cols, q, seed, check)?;
    scale_down(&mut work.scaled, &work.down, &work.smooth, cols);
    let maximum = residual_maximum(weight, rows, cols, &work.smooth, &work.up, &work.scaled, check)?;
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
    let smooth = &work.smooth;
    input_quantize(calibration, |x, j| x / smooth[j], gx, &mut work.xq, cols, check)?;
    let global = (maximum / LEVELS).max(1e-30);
    score(work, weight, calibration, reference, rows, cols, samples, true, global, None, check)
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

/// Fit a complete tensor, including alpha selection and served calibration score, on
/// the pool's workers. A single faer QR/SVD call is not internally cancellable; every
/// surrounding pass, input/output tile and alpha observes the caller's native control.
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
    check: Check,
) -> Result<SvdqFitted> {
    pool()?.install(|| {
        fit(weight, xmax, calibration, rows, cols, samples, seed, max_buffer_bytes, check)
    })
}

#[allow(clippy::too_many_arguments)]
fn fit(
    weight: &[f32],
    xmax: &[f32],
    calibration: &[f32],
    rows: usize,
    cols: usize,
    samples: usize,
    seed: u64,
    max_buffer_bytes: usize,
    check: Check,
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
        check()?;
        finite(values, name)?;
    }
    if xmax.iter().any(|x| *x < 0.0) {
        return Err(fail("channel maxima must be nonnegative"));
    }
    let reference = {
        let mut outputs = zeros(samples * rows)?;
        if nano(samples, rows, cols) {
            let mut transposed_weight = zeros(rows * cols)?;
            transposed(&mut transposed_weight, weight, rows, cols);
            exact_par(&mut outputs, calibration, &transposed_weight, cols, rows, check)?;
        } else {
            product(
                Out::row(&mut outputs, samples, rows),
                View::row(calibration, samples, cols),
                View::row(weight, rows, cols).transpose(),
                check,
            )?;
        }
        finite(&outputs, "reference outputs")?;
        let mut norms = zeros(samples)?;
        norms.par_iter_mut().enumerate().for_each(|(s, norm)| {
            *norm = outputs[s * rows..(s + 1) * rows]
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                .sqrt();
        });
        Reference { outputs, norms }
    };
    // Maxima are exact in any order: each worker's rows, then the workers'.
    let column_max = |mut maxima: Vec<f32>, values: &[f32]| {
        for (maximum, value) in maxima.iter_mut().zip(values) {
            *maximum = maximum.max(value.abs());
        }
        maxima
    };
    let wmax = weight
        .par_chunks_exact(cols)
        .fold(|| vec![0.0f32; cols], column_max)
        .reduce(|| vec![0.0f32; cols], |a, b| column_max(a, &b));
    let weight_max = wmax.iter().fold(0.0f32, |m, v| m.max(*v));
    let input_max = calibration.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // Alphas at once: one per eight workers (each alpha's QR and SVD run on one), as many as
    // the grant holds (one is `required`).
    let each = alpha_workspace(rows, cols, samples)?;
    let held = 1 + (max_buffer_bytes - required) / each;
    let inflight = held.min((threads() / 8).max(1)).min(ALPHAS.len());
    let mut works = (0..inflight)
        .map(|_| Work::new(rows, cols, samples, q))
        .collect::<Result<Vec<_>>>()?;
    let work = &mut works[0];
    input_quantize(
        calibration,
        |x, _| x,
        (input_max / LEVELS).max(1e-30),
        &mut work.xq,
        cols,
        check,
    )?;
    let plain = score(
        work,
        weight,
        calibration,
        &reference,
        rows,
        cols,
        samples,
        false,
        (weight_max / LEVELS).max(1e-30),
        None,
        check,
    )?;
    // The first alpha with the least error wins, as when they ran one after another.
    let mut best_error = f64::INFINITY;
    let mut best_alpha = 0.0f32;
    let (mut best_up, mut best_down, mut pre) = (zeros(rows * RANK)?, zeros(RANK * cols)?, zeros(cols)?);
    let shape = (rows, cols, samples, q, seed);
    for alphas in ALPHAS.chunks(inflight) {
        let errors = works
            .par_iter_mut()
            .zip(alphas.par_iter())
            .map(|(work, &alpha)| {
                candidate(work, alpha, weight, xmax, &wmax, calibration, &reference, shape, check)
            })
            .collect::<Vec<_>>();
        for ((work, &alpha), error) in works.iter().zip(alphas).zip(errors) {
            let error = error?;
            if error < best_error {
                best_error = error;
                best_alpha = alpha;
                best_up.copy_from_slice(&work.up);
                best_down.copy_from_slice(&work.down);
                for (p, s) in pre.iter_mut().zip(&work.smooth) {
                    *p = 1.0 / s;
                }
            }
        }
    }
    check()?;
    works.truncate(1);
    let work = &mut works[0];
    work.up.copy_from_slice(&best_up);
    work.down.copy_from_slice(&best_down);
    for (s, p) in work.smooth.iter_mut().zip(&pre) {
        *s = 1.0 / p;
    }
    scale_down(&mut work.scaled, &work.down, &work.smooth, cols);
    let maximum = residual_maximum(
        weight,
        rows,
        cols,
        &work.smooth,
        &work.up,
        &work.scaled,
        check,
    )?;
    let global = nvfp4::min_sse_global(f64::from(maximum))?;
    let mut codes = zeros(rows * cols / 2)?;
    let mut scales = zeros(rows * cols / 16)?;
    // Rows on the workers: each encodes its blocks and keeps its residual (in `a`) and
    // logical values; the error statistics then sum them in the sequential order.
    let mut logical = zeros(rows * cols)?;
    let (smooth, up, down) = (&work.smooth, &work.up, &work.down);
    let scaled = &work.scaled;
    let saturated = codes
        .par_chunks_exact_mut(cols / 2)
        .zip(scales.par_chunks_exact_mut(cols / 16))
        .zip(work.a.par_chunks_exact_mut(cols))
        .zip(logical.par_chunks_exact_mut(cols))
        .enumerate()
        .map(|(i, (((codes, scales), residuals), logical))| {
            check()?;
            residual_row(residuals, weight, i, smooth, up, scaled);
            let mut saturated = 0u64;
            for j in (0..cols).step_by(16) {
                let block: [f32; 16] = std::array::from_fn(|b| residuals[j + b]);
                let (scale, nibbles, count) = nvfp4::min_sse_block(&block, global);
                scales[j / 16] = scale;
                for b in 0..8 {
                    codes[j / 2 + b] = nibbles[2 * b] | (nibbles[2 * b + 1] << 4);
                }
                saturated += count;
                let scale = crate::format::decode::elem::e4m3(scale);
                for b in 0..16 {
                    let code = crate::format::decode::elem::e2m1(nibbles[b]);
                    // Match decode::plan::Post::LowRank: every stored-value
                    // operation rounds in F32; only the error accumulation is F64.
                    logical[j + b] = ((code * scale) * global) * pre[j + b];
                }
            }
            for r in 0..RANK {
                let u = up[i * RANK + r];
                for (value, d) in logical.iter_mut().zip(&down[r * cols..(r + 1) * cols]) {
                    *value += u * *d;
                }
            }
            Ok(saturated)
        })
        .try_reduce(|| 0, |a, b| Ok(a + b))?;
    let mut residual = QuantizationStats {
        saturated,
        ..QuantizationStats::default()
    };
    let mut stats = QuantizationStats {
        saturated,
        ..QuantizationStats::default()
    };
    for i in 0..rows {
        check()?;
        for j in (0..cols).step_by(16) {
            let scale = crate::format::decode::elem::e4m3(scales[i * (cols / 16) + j / 16]);
            for b in 0..16 {
                let at = i * cols + j + b;
                let code = codes[at / 2] >> ((at % 2) * 4) & 15;
                let decoded = (crate::format::decode::elem::e2m1(code) * scale) * global;
                residual.add(work.a[at], f64::from(decoded));
                stats.add(weight[at], f64::from(logical[at]));
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
    drop(logical);
    let pre = &pre;
    input_quantize(calibration, |x, j| x * pre[j], gx, &mut work.xq, cols, check)?;
    let served = score(
        work,
        weight,
        calibration,
        &reference,
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
        float_bytes(pre, false)?,
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
        workers: threads(),
    })
}
