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
