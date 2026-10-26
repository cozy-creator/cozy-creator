# Bounded faer SVD producer patch

Exact upstream faer 0.24.4, crate SHA-256
`5ab6df3dd147fe8d702a288b95bcd8fcc499ab572fc80da6828f60cd4d524d67`.
See TENSORFS-PATCH.json for every original/copied source hash and the patched SVD hash.

Four SVD block-size choices (two scratch calculations and two execution sites)
use block size 1. QR and SVD algorithms are upstream, unchanged. The additional
constant lets the TensorFS producer refuse to compile against the unpatched crate.

Reason: stock faer 0.24.4 calls its native GEMM backend while applying blocked
Householder factors. private-gemm-x86 0.1.22 keeps an infallibly allocated thread
local packing buffer sized from CPU cache, outside svd_scratch/qr scratch. The
TensorFS Writer grant cannot account for or make that allocation fallible.

The SVDQ caller is sequential F32, q <= 48, recursion_threshold=128, with
column-major bases, singular factors and library scratch. The modified block-1
Householder path and QR use matrix-vector/rank updates, and the small bidiagonal
SVD uses upstream QR (not divide-and-conquer). Explicit outer matrix products
are tiled to 16^3 operations, selecting faer's allocation-free nano-GEMM path.
The caller allocates faer MemBuffer through try_new after charging its StackReq.

This is a first portable bounded producer, not a speedup claim. No global
parallelism change, thread pool, BLAS service, QR/SVD rewrite, or decoding change.
Unit-test data are omitted from this vendored build input; source and
upstream license notices are retained. This dependency is built with only
std,linalg, default-features=false. Tests are real TensorFS release-gate cells.
