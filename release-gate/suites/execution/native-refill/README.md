

## Ready plain-factor reads

The `factor_reads` endpoint exercises the installed Runtime fast path with TensorFS0.15.8:

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/native-refill/factor_reads \
  seed=158001 --on OWNED_MACHINE --gpus=1 --json
```

It creates2,364 actual stored LoRA-shaped factors, prepares one common block, then performs60
real ready reads and dependent GPU matrix multiplications. cProfile counts must show60 `_read`
calls with zero `_ensure`/`_assign` calls and zero source bytes moved during that measured section.
Module placeholders remain identical. This is direct work-count evidence; the profiled wall time
includes Python instrumentation and is not a full-model speedup or throughput measurement.

The same request checks tied active Parameter identity, BF16 conversion, an escaped alias blocking
reclaim, a real eviction/refill through the original binding path, a side-stream consumer, and
mutable-fill invalidation retaining the old escaped bytes. Cleanup must have no remaining physical
pins or poison. The historical refill modes remain unchanged. This endpoint uses the ordinary
installed executor directly, without a candidate interpreter override, fake backend or monkeypatch.
Source checks pass; actual matched-artifact execution is required before qualification.

Real `factor_reads` run **17798 passed** on Runtimef0d25c8e7/TensorFS9e95ee586:60 reads across
2,364 factors required0 `_ensure`/`_assign` calls and moved0 source bytes; the cold refill
still called each once. GPU math and every tied/dtype/alias/side-stream/mutable-fill assertion
passed with complete pin cleanup. Profiled84.953ms covers the whole measured section, not a
per-read or model speedup. [Exact cohort](../native-load-execute/evidence/9e95-f0d-local-21.md).
