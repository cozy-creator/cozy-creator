# Native NVFP4 min-SSE encoding

Run the bounded CPU conversion cell through the installed SDK in the default Cozy home:

```sh
cozy run /absolute/path/to/nvfp4-encode/run seed=103 --on MACHINE --json
```

Requires TensorFS0.15.3's native `nvfp4-min-sse/1` strategy and the current Runtime
`derive.microscale.encode_nvfp4` reference producer. It compares actual packed weight,
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
