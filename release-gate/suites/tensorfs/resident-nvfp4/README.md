# Resident NVFP4 decode

This real CUDA cell runs through the installed executor SDK:

```sh
cozy run /absolute/path/to/resident-nvfp4/run seed=103 --gpus=1 --on MACHINE --json
```

It checks 96 combinations of F32/BF16/F16 output, low/high nibble order, multiply/divide
global scales, row-major/cuBLAS scale layouts, optional BF16 pre-quantization factors and
single/stacked matrices. Values deliberately span scale-tile boundaries and exactly
representable arithmetic. CPU TensorFS decoding and the independently calculated value
law must agree with the resident GPU output on a nondefault producer stream. Metadata,
mixed-placement and overlap refusals preserve the destination, followed by a healthy read.

This is a format/ownership correctness cell, not a quantizer quality or performance benchmark.
It does not qualify arbitrary GPU faults or Runtime provider migration. No unit tests or mocks.

Installed RTX3060 run **17463** passed all 96 conversion cases, three typed refusals and
healthy retry on 2026-10-25. TensorFS `7e98dce7f` contains the unchanged decoder implementation
from `527c1ba29`; Runtime `67e2e0c10`, TensorD `ca3849147`.
[Exact receipt and hashes](evidence/17463-rtx3060.json).
Earlier17454 failed only the fixture's expected mixed-placement error text;8c262f158 corrected
that assertion. Neither run establishes quantizer quality or native load/decode composition.

Regression **17819 passed** all96 cases on TensorFS9e95ee586/0.15.8, Runtimef0d25c8e7 and actual
TensorD4178, alongside the separately checked encoded LoadSlot matrix.
[Matched cohort proof](../../execution/native-load-execute/evidence/9e95-f0d-local-21.md).
