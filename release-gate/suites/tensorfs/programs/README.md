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

Findings (things the public API can't reach):
- No public build identity for the native library. The packages hashed `tensorfs._ext.__file__`; the programs report
  `tensorfs.__version__` instead.
- `Writer.quantize` takes and returns TypedDicts (`QuantizeTensor`, `QuantizedTensor`, `QuantizationResult`) that live in
  the private `tensorfs.api` and aren't in `tensorfs.__all__`. A caller can pass plain dicts, but it can't name the types.

The Runtime halves of the split packages (xs-174):
- offline-producers: `suites/execution/derive-adapters` (a public-API package): `cozy_runtime.derive.microscale`'s
  producers against the same frozen references. The old package ran this only with `runtime=true`, which no gate set.
- rowwise-quantize: the Runtime's `RowwiseDequant` was this program's reference; the program now holds TensorFS to the
  rowwise law itself, and the Runtime's decoders are the kernels suite's `decode:conform` cells.
