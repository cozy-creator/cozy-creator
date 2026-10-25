# Native NVFP4 min-SSE encoding

Run the bounded CPU conversion cell through the installed SDK in the default Cozy home:

```sh
cozy run /absolute/path/to/nvfp4-encode/run seed=103 --on MACHINE --json
```

Requires TensorFS0.15.3's native `nvfp4-min-sse/1` strategy and the current Runtime
historical `encode_nvfp4` reference shipped inside this gate (see ORACLE.md). It compares actual packed weight,
block-scale and global-scale bytes across logical dtypes and row chunk sizes. A common
whole-tensor maximum must preserve output even when the largest weight is in another chunk.
The existing Writer/readback and decoder check the emitted parts. Invalid statistics,
metadata, nonfinite values and failed-global-scale representations refuse before a healthy retry.

This deliberately distinguishes a production bug: Runtime can derive global scale zero for
the smallest nonzero F32 input; the new strategy refuses it. The cell records that difference
instead of calling it byte parity. Ordinary and all-zero cases must retain exact producer bytes.

Scale-candidate SSE reduction uses the same16-element pairwise order as
[NumPy's contiguous reduction](https://github.com/numpy/numpy/blob/v2.5.2/numpy/_core/src/umath/loops_utils.h.src#L88).
The fixture records the installed NumPy version and reference/native source hashes.

This is tensor arithmetic and Writer integration proof, not model quality, speed, full native
checkpoint conversion or a production producer switch. No unit tests or mocks.

Installed CPU qualification passed on2026-10-25: run **17471**, combined source7e98dce7f,
TensorFS0.15.3 wheel f80ef0d9… / native126fd414…, Runtime67e2e0c10, TensorDca3849147,
NumPy2.5.4, on RTX3060 host quatre through the default CLI. All24 logical-dtype cases,
chunk sizes1/2/7, 12 refusals and existing Writer/readback passed.
[Exact receipt](evidence/7e98dce7f-cpu.json). This does not qualify the later measured
encoder/Writer.quantize checkpoint loop or production Runtime cutover.

The TensorFS0.15.6 extension exercises both `raw.quantize` and `raw.quantize_measured` on every
case and chunk. Parts must match exactly, with native amax/max-error/saturation exact and SSE/energy
within rtol2e-13 against the independent frozen oracle. Every prior refusal must have the same
error code/detail through both APIs. Existing receipts predate this measured-API gate; a new
installed real CPU run is required.

Measured API **17747 passed** through the default CLI on the local machine:24 dtype cases,
123 measured calls including row chunks1/2/7, and12 matching refusals through both APIs.
TensorFS e521cc779/ca15dd65 native91ebdf02 and NumPy2.5.4; frozen oracleb34b3cb1. Parts,
amax/max-error/saturation are exact; SSE/energy use rtol2e-13. Whole-tensor versus chunk maxima,
explicit positive-scale-underflow refusal and Writer/readback integrity passed.
[Compact actual receipt](evidence/e521cc779-measured-cpu.json). This is CPU numerical proof;
the separately observed CLI BF16 guard refusal used a replaced controller and was not a run
of the repaired guard.
