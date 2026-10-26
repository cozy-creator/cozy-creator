# Prepared resident component owners

This is a real installed-SDK CUDA release-gate cell. It creates a small TensorFS checkpoint and
loads two components through Runtime's actual resolution, Backend and Placement. It uses no mock
allocator, fake placement or private candidate interpreter. It is a correctness gate, not a model
benchmark.

```sh
cozy run /absolute/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  seed=179601 --on local --gpus=1 --json
cozy run /absolute/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=attention seed=179602 --on local --gpus=1 --json
```

Use a new seed for each attempt; a memo hit is not new execution evidence. Root coordinates the
machine's selected Runtime and TensorFS artifacts. The cell never installs SDKs or
rents machines.

The cell requires these actual outcomes:

- Before the first forward, resident-only capture refuses without a source read or body call.
- After one eager forward, a placed module alone retains its binding; capture reads zero bytes,
  and one native graph launch runs four exact linear steps without Python continuation.
- The actual CLI executor request stays non-overlapping; the fixture does not open a nested
  non-idle measurement request. The next component produces a second
  phase transition after graph retirement, preserving phase prioritization.
- A package forward hook and the actual per-step LoRA setting refuse before warmup.
- A mutating module counter is restored with the declared state before preparation refuses.
- A real allocation request twice the device's total VRAM fails after the warmup has queued a
  state update; checked cleanup restores that state and subsequent CUDA model work succeeds.
- Publishing an actual ATen linear callable through `kernel_ready.wait` advances the real
  readiness registry. A previously captured plan refuses its stale kernel epoch with zero
  launches. This verifies the registry transition, not asynchronous bake compiler completion.
- The separate `mode=attention` call replays built-in zero-dropout attention from one native
  graph launch; actual nonzero dropout
  and random generation still refuse before replay and preserve the declared state.
  Each request submits at most one plan through the actual Context admission.

Reports include Runtime/TensorFS versions, physical device, phase changes, refusals and native
launch counts. Source formatting/lint checks are separate from the pending GPU receipt. No passing
GPU qualification or numerical parity for a production image model is claimed by the fixture.

## Minimal full-SDXL convolution capture

`mode=conv-torch` and `mode=conv-varena` isolate the first failing operator from full-SDXL
run17721 without downloading a checkpoint. They are **two separate CLI invocations**:

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=conv-torch seed=1772101 --on MACHINE --gpus=1 --json
cozy run /absolute/canonical/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=conv-varena seed=1772101 --on MACHINE --gpus=1 --json
```

Use the same fresh seed for both backing modes to keep tensor bytes identical. Mode is part of
the request; a repeated successful mode/seed could memo-hit. Preinstall separately if needed:
`cozy run <suite>/gpu --warm installed --on MACHINE --json` (no `--gpus` with `--warm installed`).

Both modes run actual FP16 `aten.convolution.default` with contiguous input `[2,4,128,128]`,
weight `[320,4,3,3]`, bias `[320]`, stride/dilation1, padding1 and groups1. The sequence is an
ordinary eager call, warmup on a fresh CUDA stream, manual global capture on that same stream,
then one actual graph replay checked against eager at `rtol=atol=1e-3`. There is no cache flush
or device-wide synchronization inserted between warmup and capture. Existing cuDNN settings
are reported without overriding them.

The Torch mode uses ordinary Torch allocations for weights and bias. The Varena mode directly
reserves and fills real native regions and retains their uses, tensor aliases and physical pins
through replay and graph retirement. Both modes use the same native capture guard. This
isolates backing ownership; it is not a disk-loading, allocation-recovery or performance test.

The Linux-only diagnostic resolves `cuStreamIsCapturing` before capture and records its raw
return/status around the convolution. Memory snapshots run outside capture. Failure notes carry
bounded metadata through the normal Invoke traceback; they include cuDNN settings, tensor
strides and the original error if capture-end/reset also fails. Success reports executor PID,
versions, one manual graph launch, maximum output difference and actual Varena counters.
Separate invocations may reuse an executor; use the reported PIDs when interpreting cuDNN cache
history. This gate does not submit a PreparedRepeat or prove full-model success.

Source-only checks do not qualify either mode on GPU. Root owns all actual machine/rental runs.

### Actual telemetry watcher during capture

`mode=conv-watched` keeps the Torch-backed convolution and manual global capture unchanged,
but constructs the real injected `Telemetry.step_callback(2, stage="convolution")` before
the eager call, reports step0, and leaves the actual `cozy-host-stalls` sampler running. It
pauses the host for0.3seconds after capture begins, then checks raw capture status before
launching the convolution. Step1 is reported after successful replay, allowing the normal
service to stop its watcher. On failure the ordinary attempt lifecycle owns watcher cleanup.

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=conv-watched seed=1772601 --on MACHINE --gpus=1 --json
```

The fixture reads, but never modifies, the service-created watch's existing sample counters
and records its thread presence before/after the pause. No idle callback, CUDA query or error
is replaced or injected. Elapsed time alone does not prove the sampler executed: inspect the
counter change, thread presence and raw before/after status together. A query that raises can
end the watcher before a sample is counted.

This is a reproduction hypothesis, not an asserted CUDA violation. NVIDIA documents that
legacy-stream execution queries conflict with capture of a **blocking** stream, while Torch's
stream pool uses `cudaStreamNonBlocking`. The fixture preserves that actual nonblocking stream;
it does not switch to a blocking stream to manufacture a failure. See the
[NVIDIA capture restrictions](https://docs.nvidia.com/cuda/cuda-programming-guide/04-special-topics/cuda-graphs.html)
and [pinned Torch stream flags](https://github.com/pytorch/pytorch/blob/5c4886908584029761b579af026dcfb627c84070/c10/cuda/CUDAStream.cpp#L21).
Source checks do not establish the cause or a passing GPU result.

### PreparedRepeat capture with the real watcher

`mode=conv-prepared-watched` is the production-path regression for observed run17743, where
the manual watched fixture changed capture status from1 to2 before its convolution. It uses
the same actual FP16 convolution with Torch-owned weights, a real injected
`Telemetry.step_callback`, and `PreparedRepeat.capture`. Its Python body pauses0.3seconds
**only during CUDA capture**, checking raw driver capture status before and after the pause.
Warmup does not pause; GPU replay makes no Python calls.

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=conv-prepared-watched seed=1774301 --on MACHINE --gpus=1 --json
```

Run the exact fixture against the unfixed Runtime first and retain the red receipt, then use
a fresh seed on the fixed candidate. The passing criterion is actual successful capture,
a resumed sample count from the real watcher after capture, exactly one `ctx.plan` submission
and native host graph launch, two GPU iterations, numerical agreement with eager, and normal
final-step watcher shutdown. Watch counters are inspected only **outside capture**, so the
fixture does not contend recursively with the production pause lock. The resume observation
has a1second deadline; elapsed time alone is not success.

No callback is replaced, no exception is injected, and no mock allocator or model is used.
Failure notes retain the raw capture observations. The original manual `conv-watched` mode
remains unchanged as a diagnostic and is not expected to be protected by PreparedRepeat's
production capture scope. Source checks are separate from the required real red/green runs.

## Final combined candidate controls

[Creator8499 / Runtime7d1 / TensorFS9e95 real controls](../native-load-execute/evidence/8499-7d1-integrated-controls.md) preserve the exact25October2026 GPU receipts and link the separate recipe proof. This control proof does not claim full-model native29 completion or a performance comparison.

## Native graph-inspection refusal

`mode=inspection` uses an actual pinned CPU tensor and a CUDA state tensor. The real
`PreparedRepeat.capture` body performs `state.copy_(host, non_blocking=True)` in warmup and
capture. The native inspector must refuse that host-to-device graph before Context submission,
with typed `prepared_graph_ineligible` at the source stage. It accepts only the exact host-copy
representations: `memcpy_layout_or_kind` with CUDA kind `1` (HostToDevice), or `memcpy_source`
with kind `4` (Default) and pointer type `1` (Host). It verifies restored state and healthy CUDA, then
submits one ordinary device-only plan: two iterations, one native launch, exact output.

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  mode=inspection seed=1788201 --on MACHINE --gpus=1 --json
```

This requires the native diagnostic artifact, not an old wheel with the same Runtime version.
No exception, node type, callback or allocator is injected. If Torch cannot capture this actual
copy on a selected system, the cell fails visibly; it must not force a synthetic inspector
failure. Source checks are separate from the required real SDK run. This H2D refusal does not
identify the cause of the earlier full-SDXL graph refusal or authorize broader graph admission.

[Actual db7 native qualification](evidence/db7-native-inspection-local.md) passed the real
H2D refusal and healthy follow-up in 17892, watched capture in 17893, and raw native
loading/continuation in 17895. Normal restore 17896 returned the known healthy SDK.
