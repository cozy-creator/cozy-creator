# Native weight residency release gate

Use the default Cozy CLI and the machine's installed candidate SDK:

```sh
cozy --json run ./release-gate/suites/varena/stock-memory/native seed=741 --gpus=1 --on OWNED_MACHINE
cozy --json run ./release-gate/suites/varena/stock-memory/gpu seed=742 --gpus=1 --on OWNED_MACHINE
cozy --json run ./release-gate/suites/varena/stock-memory/driver seed=743 --gpus=1 --on OWNED_MACHINE
cozy --json run ./release-gate/suites/varena/stock-memory/derived seed=744 --gpus=1 --on OWNED_MACHINE
```

`native` proves real Rust CUDA VMM ownership and native allocation recovery: 64 MiB weight
backing, a 192 MiB ordinary Torch worker-thread allocation under a 224 MiB activation admission
limit, exactly enough inactive-group reclaim, escaped aliases, stock cache reuse, pointer-stable
refill, failed-fill exclusion, real graph capture/replay, and typed budget/driver OOM. The oversized
CUDA request exceeds card capacity and fails immediately; it does not allocate VRAM ballast or
induce a sticky device fault. It records the extension hash and observed driver-free bytes.

`gpu` exercises actual TensorFS files through Runtime's weight consumer, finite streaming,
escaped aliases, refill, graph preparation/replay/recapture, explicit pure-cut recovery, and a real
float64 binder on side streams. `derived` compares ordinary warm calls against an explicit pinned
baseline for plain float32, float8-to-float32 decode, and derived transpose. It reports binder calls,
store bytes, CUDA timing and wall timing. These are mechanism measurements, not full-model speed
or quantized-model performance claims.

`driver` additionally proves partial physical reclamation while a graph remains pinned, and
GPU fill-identity changes preserving old escaped aliases. The Linux cancellation controller
submits its own run, waits for a pinned native ready receipt, cancels that exact run through the
CLI, verifies the child's native cleanup receipt and process birth/exit, and preserves its log.

The owned child uses the executor's interpreter when `candidate_python` is omitted. Current
qualification must leave that historical override empty and record the installed SDK hashes.
These mechanism gates are not public release, Windows GPU qualification, driver-wide allocation
coverage, or a hard total-device memory cap. Root alone coordinates machine SDK changes.

The TensorFS0.15.8 `native` extension also checks `Region.pin()` against real VMM storage:
cold/loading misses leave residency unchanged, a ready pin owns the original address without
another fill, and an escaped alias prevents reclaim after its `Use` is dropped. A real side-stream
reduction remains correct across subsequent checked reclaim, an evicted pin returns `None`
without remapping, and normal refill produces new bytes at the same address. Final pins and
resident bytes return to zero without poison. This new ready-pin result needs a fresh matched
artifact run; older receipts do not qualify it. No mocked driver or replacement allocator is used.

The extended native cell **17796 passed** with TensorFS9e95ee586/0.15.8 and Runtimef0d25c8e7.
Ready-only misses, alias protection, side-stream completion, eviction and same-VA refill all
passed with zero final pins/resident bytes and no poison. Existing allocator/graph assertions
also passed. [Matched cohort proof and limits](../../execution/native-load-execute/evidence/9e95-f0d-local-21.md).
