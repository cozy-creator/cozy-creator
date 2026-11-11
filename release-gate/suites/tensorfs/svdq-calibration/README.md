# SVDQ calibration and the adopted component

A real GPU proof on the Runtime's public API: an authored BF16 Linear (128 outputs, 256 inputs), real calibration hooks
over eight known batches, the public Output conversion with the native fitter, adoption, and a separately loaded
encoded consumer. `run.py` drives it through the ordinary CLI (the execution suite's `quantize_proof`):

```sh
python3 release-gate/suites/tensorfs/svdq-calibration/run.py \
  --machine OWNED_MACHINE --model fidika/unique-svdq-proof --seed 156001 --out /absolute/receipts
```

| call | what it proves |
|---|---|
| `components`, `resume` | the Runtime's per-component conversion across an Output's lifetime: one plan identity, finished reuse with no reads or writes, resume after a close, identity refusals. They reopen the Runtime's own Outputs, so they are its built-in `bench.quantize_reuse`, which these call |
| `source` | the private BF16 source model, uploaded |
| `calibrate` | measured input maxima and 128 captured rows; SVDQ without calibration refuses before writing; the native fit's calibration error beats plain NVFP4 and stays within the frozen Torch fitter's (`historical_svdq.py`) |
| `predict` | through its own grant of the adopted artifact (`ctx.tensorfs_source`): every stored part is exactly what the frozen NVFP4 reference packs from the selected residual (`checks.py`, `oracle.py`), the bias is inherited, and two held-out batches match the stored value law (relative error 0.02 decoded, 0.10 native) and the original BF16 output (0.20) |

On cards before SM100 the consumer decodes the weight (`decoded-weight-gemm`); `--require-native` demands the native
SVDQ provider. TensorFS's own fitting, refusals, cancel and resume are the program `../programs/svdq_native.py`.
`ORACLE.md` says where the frozen references come from; `evidence/` holds earlier receipts.

Findings (xs-174): calibrate can't read back the artifact it adopts, so predict checks the stored parts, with the fit's
row passed from calibrate; the served-score check needs the calibration rows, so it runs only in the program. No public
build identity for TensorFS's native library: the provenance names `tensorfs`'s version.
