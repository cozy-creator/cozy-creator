# Native load → execute: real release gate

This fixture qualifies a declared raw input slot feeding an already captured, fixed CUDA body.
It does not claim arbitrary model capture or graphs that survive eviction. TensorFS supplies
actual stored bytes; Varena owns the dedicated allocation; native code must finish the read,
retain its physical pin and launch the existing GPU repeat without a Python continuation.

The gate will run through the default Cozy CLI with `--gpus=1` against the installed candidate
Runtime/TensorFS cohort. It does not install SDKs, rent machines or run comparative benchmarks.

Required cells:

- A real file-backed weight read after capture, one `ctx.plan`, and a parked Python submitter.
  The result must depend on the nonzero loaded bytes, with preparation-only Python body calls
  and exactly one actual graph launch.
- Continuous pin ownership through load/compute and safe release after checked completion.
- Read failure from a separately copied, owned truncated file; zero graph launches and healthy
  subsequent work. Shared store objects and hard links must never be modified.
- Cancellation before launch, racing launch and during GPU execution, with checked completion.
- Normal request completion and exception unwind join the submitted native work.

```sh
cozy run /absolute/worktree/release-gate/suites/execution/native-load-execute/gpu \
  mode=loaded seed=207601 --on local --gpus=1 --json
```

Run `loaded`, `cancel-before`, `cancel-race`, `cancel-active`, `input-error`, `scope-return`
and `scope-unwind` as separate requests with distinct seeds. Use a new seed for any repeat;
an application memo hit is not new GPU proof. The fixture runs in the installed executor SDK,
without spawning a private candidate interpreter. It refuses a missing `LoadSlot` capability.

The body adds the sum of a real 64 MiB F32 weight to one state tensor. The capture slot starts
at zero; TensorFS loads the nonzero file only after capture. Thirty steps must produce the
exact expected sum with only two Python preparation calls and one actual native graph launch.
`loaded` requires completion during a three-second park before its first observation. This
is a budget for the known small fixture, not a general hardware latency guarantee or a
performance acceptance bar. No Python poll advances the read or launch during the park. Long
cancellation loops are finite and cancellation must finish before their natural step limit.
`cancel-active` waits for the native worker's `compute_wait_entered` fact, starts an independent
completion observer, then cancels; it cannot pass solely by canceling before launch.

The scope/error cells use the real public `App`/`Invocation`/`attempt` lifecycle inside the normal
GPU request, following the existing qualified resident-lifecycle gate. This contains the expected
author/source failures so their native completion/launch facts can be asserted and returned.
Other cells submit through the request's supplied `Context`. The input-error cell independently
copies a 256 MiB checkpoint, verifies each source file has one link, and truncates only its own
last chunk after actual I/O starts. It refuses a missed damage window instead of claiming proof.

Reports include package versions, device, source SHA, successful and issued read bytes, native
stage timestamps, actual graph launches, cancellation/progress, pin reclamation and checked
completion. The source identity, native read and result are separate facts. This gate measures
correctness, not model throughput or comparisons with ComfyUI.

The installed RTX 3060 cohort passed all seven modes on 2026-10-25: runs **17446–17452**,
Runtime `c4233c9d6`, TensorFS `527c1ba29`, TensorD `ca3849147`. The loaded case consumed
64 MiB and completed all 30 reductions with one graph launch during a three-second Python
park. Active cancellation stopped after 103 steps; failed reads launched no graph and left
the context healthy. Scope return joined 2,000 steps; exception unwind canceled and joined.
All modes checked physical pin/alias cleanup. Exact artifacts, facts and qualification
limits are in [the receipt](evidence/c4233c9d6-rtx3060.json).

These are mechanism correctness results. They do not establish a production denoiser,
encoded load-to-execute composition, GPU-only transfer handoff or comparative speed.


Current invocation API refresh: all seven modes passed again through the default CLI on the
RTX3060 (runs17556–17562; exact per-mode run numbers in the receipt), Runtime de3dfc42c,
TensorFS6431a5387 and TensorD9998cf5f4. The fixture uses the executor's actual
prepare/run_prepared boundary; source8176adc6a. [Exact cohort and receipts](evidence/de3dfc42c-rtx3060.json).
This repeats raw loading, cancellation, failed-source refusal, checked request completion and
alias/pin cleanup. It does not qualify encoded LoadSlot composition or event-only transfer handoff.
