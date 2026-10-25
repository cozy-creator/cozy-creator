# Native weight residency release gate

Use the default Cozy CLI and an explicitly identified candidate interpreter:

```sh
cozy --json run ./release-gate/suites/varena/stock-memory/native seed=741 candidate_python=/absolute/candidate/bin/python --gpus=1 --on local
cozy --json run ./release-gate/suites/varena/stock-memory/gpu seed=741 candidate_python=/absolute/candidate/bin/python --gpus=1 --on local
cozy --json run ./release-gate/suites/varena/stock-memory/driver seed=741 candidate_python=/absolute/candidate/bin/python --gpus=1 --on local
cozy --json run ./release-gate/suites/varena/stock-memory/derived seed=741 candidate_python=/absolute/candidate/bin/python --gpus=1 --on local
python release-gate/suites/varena/stock-memory/cancel_gate.py --candidate-python /absolute/candidate/bin/python --output /absolute/owned/cancel-proof
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

The candidate child imports a separately built TensorFS extension and Runtime wheel; the outer
executor can remain on the machine's installed SDK. This is an ordinary-CLI component/consumer
proof, not a claim of installed cohort adoption, public release, Windows GPU qualification,
driver-wide allocation coverage, or a hard total-device memory cap. No shared SDK update is
needed to run it.
