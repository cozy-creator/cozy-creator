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
