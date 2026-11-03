# Native Ulysses host and full-model qualification

`host.py` uses actual Torch Gloo process groups with two and four ranks, actual
ProcessGroup Store messages, supplied all-to-all receivers and production packing/
assembly functions. It covers unequal and zero local row counts and head padding.
The arithmetic control is real Torch Q+K+V; it does not emulate a native GPU provider.
`spans.py` uses real distinct frombuffer Storage objects to check contiguous and
strided-source physical overlap refusal before writes.
`eager.py` checks actual ordinary Conv3d/RMSNorm/Diffusers FFN execution and authored
fixed windows after removal of the Python capacity/retry ladder.

Run with the exact candidate Runtime/TensorFS installed and CPU Torch2.15.0
(gitd0f2af8b66b20da4baa50644c0cd3f887a2eae2e), with one CPU and private caches:

```sh
OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 \
  CUDA_VISIBLE_DEVICES='' nice -n19 taskset -c 0 python host.py
OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 \
  CUDA_VISIBLE_DEVICES='' nice -n19 taskset -c 0 python eager.py
```

These clients do not qualify CUDA, NCCL, peer copy engines, attention kernels,
quality or performance. The operator must run the actual unchanged H3 request at
15seconds,362frames,1344x768,30steps and audio, with `--gpus=2` and `--gpus=4`,
using the current paired Runtime/native SDK and the selected supported Kitchen
recipe. Preserve exact artifact/profile receipts, every rank's native provider,
choice/admission/readiness records, final output and failure receipts. Repeat on
an independent GPU allocation. Five-second smoke is not acceptance.

The thirteen real supplied-block/T->S custody regressions live in the neighboring
`vae-sequence` suite. They supplement the full model gate and do not replace it.
No GPU job or rental is launched by these host clients.
