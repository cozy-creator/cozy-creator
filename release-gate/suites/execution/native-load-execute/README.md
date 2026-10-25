# Native load → execute: real release gate

[New native diagnostic module regression](../prepared-resident-owners/evidence/db7-native-inspection-local.md)
passed raw 64 MiB loading followed by 30 GPU iterations from one launch in run 17895,
with checked completion and zero resident/pinned cleanup. This db7 cohort is separate
from the earlier raw/encoded matrix and the full-SDXL proof.

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
RTX3060 (runs17557,17558 and17560–17564; exact per-mode run numbers in the receipt), Runtime de3dfc42c,
TensorFS6431a5387 and TensorD9998cf5f4. The fixture uses the executor's actual
prepare/run_prepared boundary; source8176adc6a. [Exact cohort and receipts](evidence/de3dfc42c-rtx3060.json).
This repeats raw loading, cancellation, failed-source refusal, checked request completion and
alias/pin cleanup. It does not qualify encoded LoadSlot composition or event-only transfer handoff.

## Checked encoded load composition

TensorFS0.15.7 adds an encoded factory while preserving the raw constructor and continuation ABI.
The existing worker reads stored parts, checks copy completion, runs the existing native decoder,
checks decode completion, commits the final Fill, and only then launches the dependent graph.
These new cells are source-ready; previous raw receipts do not qualify them.

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/native-load-execute/gpu \
  mode=encoded-nvfp4 dtype=BF16 seed=157001 --on OWNED_MACHINE --gpus=1 --json
```

Run `encoded-nvfp4` and `encoded-convrot` with each of `F32`, `F16` and `BF16`, with fresh seeds.
Each uses a real checkpoint, zero-initialized typed final storage, a three-step captured row
reduction and a parked Python thread. The independent reference evaluates stored value equations
with the format's explicit F32 rounding boundaries before the output dtype conversion; it never
calls the production decoder. Final tensor values and dependent results must agree, with one
native launch and only warmup/capture Python body calls. Facts must show stored read bytes,
completed decoded bytes, bounded requested scratch, and read→decode→launch timestamps.

Separate modes `encoded-cancel-before`, `encoded-cancel-race`, `encoded-input-error`,
`encoded-decode-error` and `encoded-refusals` use actual cancellation, an independently copied
and truncated weight chunk, and a supported grouped encoding with a NaN relative scale that
triggers the real decoder error word. Failed reads/decodes must launch no dependent graph;
semantic failures must leave healthy CUDA work and checked owner cleanup. Constructor checks
cover plain/missing tensors, unsupported dtype, a valid storage format without a resident source
plan, and an actual zero scratch grant. Escaped aliases must retain the final physical pin.

The scratch bound counts aligned packed parts plus requested decoder-private buffer sizes;
it excludes final storage, shared read staging, CUDA modules/driver and allocation rounding.
It is not a hard VRAM ceiling. Cancellation does not preempt an already running decoder kernel;
the checked chain still joins it before cleanup. No event-only handoff, performance result or
full-model qualification is claimed by this gate. Repeat the seven raw modes on the new artifact,
plus the existing resident NVFP4 and ConvRot primitive/readback gates for shared decoder coverage.

Installed candidate qualification now **passed all21 cells**, runs17796,17798,17800–17817 and17819,
on Runtimef0d25c8e7/TensorFS9e95ee586 with actual TensorD4178. This includes all6 encoded dtype
combinations, actual read/decode failures with0 launches, encoded cancellation, all7 raw lifetime
regressions, ready-pin/factor-read gates and96 resident-decoder cases.
[Exact acceptance and limits](evidence/9e95-f0d-local-21.md). Encoded cancellation raced before I/O;
active-decode cancellation, broader lifecycle/platform and release acceptance remain separate.
