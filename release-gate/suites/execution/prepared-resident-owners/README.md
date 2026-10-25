# Prepared resident component owners

This is a real installed-SDK CUDA release-gate cell. It creates a small TensorFS checkpoint and
loads two components through Runtime's actual resolution, Backend and Placement. It uses no mock
allocator, fake placement or private candidate interpreter. It is a correctness gate, not a model
benchmark.

```sh
cozy run /absolute/worktree/release-gate/suites/execution/prepared-resident-owners/gpu \
  seed=179601 --on local --gpus=1 --json
```

Use a new seed for each attempt; a memo hit is not new execution evidence. Root coordinates the
machine's matching Runtime 0.32.2 and TensorFS 0.15.2 artifacts. The cell never installs SDKs or
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

Reports include Runtime/TensorFS versions, physical device, phase changes, refusals and native
launch counts. Source formatting/lint checks are separate from the pending GPU receipt. No passing
GPU qualification or numerical parity for a production image model is claimed by the fixture.
