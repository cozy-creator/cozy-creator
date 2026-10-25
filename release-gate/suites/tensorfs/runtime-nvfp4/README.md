# Runtime NVFP4 decoder and retained final buffers

Run through the installed candidate SDK and the ordinary CLI:

```sh
cozy run /absolute/worktree/release-gate/suites/tensorfs/runtime-nvfp4/gpu \
  seed=103 --on MACHINE --gpus=1 --json
```

The gate checks48 CPU/CUDA CoreDequant cases against the existing eager format core: both
nibble orders, both scale layouts, multiply/divide global scales, optional pre-quant factors,
and three destination dtypes. This tests Runtime's packed-F4 logical descriptor and metadata
handoff, beyond TensorFS's separate raw resident decoder gate.

A real encoded checkpoint then passes through production resolve/Backend/Placement into an
ordinary Linear with decoded_float selected explicitly. It requires one cold final fill,
30 warm final hits with no source reads or repeated final fills, exact repeated matmul output,
escaped alias retention, eviction and one checked refill. The installed TensorFS native hash
participates in the reported provider revision.

The fixture explicitly consents to stored encoded weights in resolve, fit and materialize,
and offers only existing decoded_float providers for this cell. Each warm iteration releases
the fixture's Placement hold before the next bind, while retaining the escaped storage alias;
this exercises final-buffer cache hits rather than repeatedly using one still-bound view.

This does not implement automatic shape-dependent packed-versus-float promotion, prove a
Blackwell kernel route, or benchmark model throughput. Unsupported targets retain their
existing Runtime decoder; this GPU gate requires a TensorFS catalog-supported target.

Installed RTX3060 run **17554 passed** on Runtime `de3dfc42c`, TensorFS `6431a5387`0.15.4
and TensorD `9998cf5f4`. All48 provider configurations matched bitwise on CPU and CUDA.
The retained case read37,384 logical bytes once, filled the final buffer once, and recorded
30 warm final hits with no additional reads/fills. Alias retention, eviction and one checked
refill passed; refill read37,384 bytes and filled once. See [exact evidence](evidence/run-17554.json).
This is format/ownership correctness, not model throughput or automatic route selection.
Run17520 completed the provider comparisons but refused the retained fixture's missing encoded
consent before materialization. Its receipt remains preserved beside the new proof metadata;
no production consent rule was relaxed.
