# Actual phase-region intent proof (unrun source)

The fixture uses the executor's actual Request/Memory arena. Native cases compare
currently resident weights with the full copied Region set at two managed capacities,
exercise generation refusal before execution, alias deduplication and explicit
retirement. Runtime cases use an actual file checkpoint and NativeLinear consumer;
the first incomplete pass is unplanned, and subsequent established regions activate
the native intent without materializing all weights. No working execution key or
eager-prefetch eligibility is claimed.

Root supplies strict actual wheel/module/GPU identities and owns GPU execution.
No mocked CUDA owner, source or fabricated Context is allowed. Source/CPU checks
are not GPU acceptance; full model no-worse gates remain open.

Run `runtime` first, then `lifetime`, then `primitive`, followed by `cleanup` using
primitive's exact returned PID/keys. The cross-arena exclusion case is last because
foreign reservation teardown follows native ownership. `runtime` verifies its model
keys are retired at model close; it does not invent request-owned keys.

`lifetime` runs real native queries on another thread while the last external Plan
owner disappears. Its 64 reservations have no separate Region owner. The fixture
waits for actual native intent/reservation retirement and records progress; this is
bounded concurrent stress, not proof of every scheduler interleaving. No error or
kernel is mocked.

CPU source proof loaded all 33,587,200 stored bytes through an actual checkpoint and
Runtime hold into eight genuine NativeLinear modules, with CUDA uninitialized.
Native source+binding cargo checks and paired Runtime mypy11→11/Ruff12→12 pass with
no new findings. GPU proof is still pending. Use `prepare.py` after fixture freeze
with the actual Runtime190fe and TensorFS4dcf wheels and exact GPU UUID; it writes
an immutable project/input/provenance set in a fresh output directory.
