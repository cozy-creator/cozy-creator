# TensorFS programs

TensorFS's own tests, run as plain programs on the gate's rental by `release-gate/programs.sh` (xs-174). They test the
library, so they aren't packages: the package contract keeps packages off TensorFS. Each program uses only TensorFS's
public API (`tensorfs`'s `__all__`, `tensorfs.raw`, `tensorfs.varena`), on the TensorFS the machine installed, which is
the version that ships. Each prints one `ROW {"cell", "result", "detail"}` line per verdict.

| program | what it proves | was |
|---|---|---|
| `resident_nvfp4.py` | resident NVFP4 decode, 96 layouts and dtypes, bit-exact against the CPU decode and the value law; refusals before any write | `tensorfs/resident-nvfp4` |
| `nvfp4_encode.py` | the NVFP4 min-SSE encoder against an independent reference (`nvfp4_encode.ORACLE.md`) | `tensorfs/nvfp4-encode` |
| `convrot_encode.py` | the int8 ConvRot encoder against an independent reference (`convrot_encode.ORACLE.md`) | `tensorfs/convrot-encode` |
| `checkpoint_quantize.py` | `Writer.quantize`'s bounded conversion, exact bytes against the references, and Writer resume after a cancel (modes basic, cancel, cancel-write; `checkpoint_quantize.ORACLE.md`) | `tensorfs/checkpoint-quantize` |
| `bf16_normalize.py` | BF16 normalization (`Writer` conversion), exact bytes against the reference, refusals, mixed resume; cancel and resume (modes basic, cancel; `bf16_normalize.ORACLE.md`) | `tensorfs/bf16-normalize` |
| `rowwise_quantize.py` | rowwise FP8/INT8 source decode bit-exact against the rowwise law (`weight * scale` in f32, rounded once), native checkpoint quantization, cancel and resume (modes basic, cancel) | `tensorfs/rowwise-quantize` |
| `offline_producers.py` | native FP8 rowwise and fixed-codebook W4A8 producers against frozen references; Writer bounds; cancel and resume (modes basic, cancel-fp8, cancel-w4a8) | `tensorfs/offline-producers` |
| `svdq_native.py` | native SVDQ fitting (`svdq-nvfp4-r32-faer/1`) on stored checkpoints: F32/F16/BF16, zero, rank-deficient and outlier weights, every selected residual packed exactly as the independent NVFP4 reference packs it (`nvfp4_encode_oracle.py`), the stored value law, the calibration score under the frozen serving law, an exact rank-32 SVD bound, bytes identical under two workspace grants, 15 refusals; a cancel held inside a fit (SIGSTOP), then resume; empty NumPy DLPack imports (modes basic, cancel, empty-dlpack) | `tensorfs/svdq-calibration`'s `native` and `empty_dlpack` |

Findings (things the public API can't reach):
- No public build identity for the native library. The packages hashed `tensorfs._ext.__file__`; the programs report
  `tensorfs.__version__` instead.
- `Writer.quantize` takes and returns TypedDicts (`QuantizeTensor`, `QuantizedTensor`, `QuantizationResult`) that live in
  the private `tensorfs.api` and aren't in `tensorfs.__all__`. A caller can pass plain dicts, but it can't name the types.

The Runtime halves of the split packages (xs-174):
- offline-producers: `suites/execution/derive-adapters` (a public-API package): `cozy_runtime.derive.microscale`'s
  producers against the same frozen references. The old package ran this only with `runtime=true`, which no gate set.
- svdq-calibration: the package keeps calibration through a model's hooks, the public Output conversion and the
  held-out GPU consumer, all on the public API; its Output reuse and resume cells are the Runtime built-in
  `bench.quantize_reuse`. The frozen Torch fitter, a quality baseline, is the package's (calibrate compares it).
- rowwise-quantize: the Runtime's `RowwiseDequant` was this program's reference; the program now holds TensorFS to the
  rowwise law itself, and the Runtime's decoders are the kernels suite's `decode:conform` cells.
