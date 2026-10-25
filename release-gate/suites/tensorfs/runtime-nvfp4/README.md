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

Source checks are not execution proof. No passing installed GPU run has been recorded yet.
Run17520 completed the provider comparisons but refused the retained fixture's missing encoded
consent before materialization. Its receipt remains preserved. The fixture-only correction
requires a fresh real run; no production consent rule was relaxed.
