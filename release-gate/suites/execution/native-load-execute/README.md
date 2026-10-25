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
`loaded` requires completion during a one-second park before its first observation. Long
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

Fixture syntax, formatting and lint checks pass. Installed-cohort GPU receipts remain pending;
do not run until the source owners provide matching Runtime/TensorFS artifacts and the root
agent coordinates the GPU lane. This PR makes no passing GPU claim yet.
