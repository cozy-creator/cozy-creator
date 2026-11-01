# Actual fused norm split consumer

This is an ordinary Cozy GPU application. It exercises the installed Runtime
adapter through a real fused `torch.nn.Module`, the actual compiled `_Launcher`,
and the executor's own `Memory.Request` and `denoise_loop` eager phase. It does
not construct or monkeypatch a Request/Context, mock a launcher, or bypass an
existing model package.

## Why this caller is real

The normal GPU executor opens `Memory` in `_open_memory`, and wraps invocation
in `host.memory.request` plus `execution.serving`. The fixture verifies that live
Request's identity against the injected public Context. `denoise_loop(range(1))`
uses the normal eager path and its existing final stream synchronization. Since
this is a componentless call, the overall phase key remains unknown, while the
closed norm leaf has a fully specified key and can publish after that actual
completion. Its keys belong to the request and must retire when it ends.

The globally declared public `fusion.ChannelNorm` registers its kernel set for
normal executor preparation. `fusion.install_norms` receives a public `Plan`
using the real `triton_kernels.load` implementation; it must install the actual
compiled `_Launcher`, never a replacement function. No handler compiles kernels
itself, and no model weights are downloaded.

## Cases

BF16 `[4,64,256,256]` channels-last input is genuinely noncontiguous in the
channel-first contract. The adapter's new full output is 32 MiB; divisor batch
copies are 32/16/8 MiB. After real preparation and trim, managed headroom is set
to 64/48/40 MiB and the fixture requires the actual selected IDs
`batch:4`, `batch:2`, `batch:1`. Native terminal leaf rows must be published after
the phase completes, while the unknown parent phase does not publish a watermark.

At 39 MiB, selection must raise its typed no-retry exhaustion before allocating
an output or copy. The actual allocator allocation count must not increase and
the operation must publish no output. This checks the real adapter path; no fake
operation-invocation counter substitutes for it.

All successful outputs must be finite and match the whole-kernel result using
Torch's dtype-default `assert_close`. Exact bytes and maximum absolute differences
are reported descriptively. This is operator/admission proof, not full-model
quality or performance qualification.

The fixture releases its tensor owners and verifies return to its pre-fixture live
allocation baseline, with no pending bytes or extra graph pools. It does not
destroy borrowed library handles or assert that the executor's CUDA context is
zero. A second `cleanup` call in the **same PID** verifies the prior request's
semantic keys were already retired. A new executor is not cleanup evidence.

The native `split_policy` fixture separately covers generic menu/lifecycle cases;
this application adds actual consumer execution and Request ownership.

## Root invocation

Install the intended Runtime split candidate and TensorFS split candidate on the
explicitly owned rental. Keep the project frozen and write payload/logs elsewhere:

```bash
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl \
  GPU-OWNED-UUID /absolute/results/norm-payload.json
cozy run /absolute/frozen/leaf-consumer/gpu --on OWNED_MACHINE --gpus=1 \
  --input /absolute/results/norm-payload.json --json
cozy run show RUN_NUMBER --json > /absolute/results/norm-run.json
```

Read the completed result's JSON `detail`; preserve the complete receipt. To make
the cleanup payload, copy the first payload and add `expected_pid` from
`detail.provenance.pid` and `keys` from `detail.request_owned_keys`. Then run:

```bash
cozy run /absolute/frozen/leaf-consumer/cleanup --on OWNED_MACHINE --gpus=1 \
  --input /absolute/results/cleanup-payload.json --json
```

Require both the CLI's successful completion and the fixture verdicts. The source
and wheel hashes are checked inside the real executor before cases begin.
Dependencies reuse the P0 production Torch 2.14.0 lock; the explicit machine SDK
override selects the exact candidate Runtime/TensorFS and the payload verifies it.

## GPU qualification: 2026-11-01

Runs **24899** (GPU cases) and **24900** (cleanup) passed on the same executor
PID **41605**. The tested fixture is commit `6706b06bd`; Runtime is
`97706e71` / `0.36.18+dev.g97706e71`, and TensorFS is `e5418c21` /
`0.17.16+dev.ge5418c21`. Exact module/extension/shim hashes, the GPU UUID,
and complete fixture results are in [the evidence receipt](evidence/2026-11-01.json).

- 64/48/40 MiB selected `batch:4`, `batch:2`, `batch:1`, respectively.
  Each exact leaf record published after the real eager phase completed.
  All output comparisons passed with maximum absolute difference zero.
- 39 MiB refused before allocation: 41,943,040 bytes required,
  40,894,464 available, `varena_no_retry=True`, allocation delta zero.
- Initial and final live bytes were exactly **17,039,360**, with two live
  executor library allocations, zero pending bytes, and no graph pools.
- The next request in the same PID found the prior request's key already
  retired (`phase_forget` returned false).

Earlier failed runs remain preserved: 24891 exposed passing the public
`author.Device` directly to Torch; 24894 exposed the fixture taking its baseline
while the executor's primed int8 library probe still ran on a background thread.
The fixture now asks the existing `int8_linear.native` readiness boundary before
measuring its baseline. This joins that real probe and records before/after
stats and availability; it neither clears borrowed library workspaces nor relaxes
the strict final equality assertion.

AST, Ruff, schema and offline-lock checks also passed. This qualifies the actual
closed norm adapter and request cleanup on the recorded cohort. It does not
qualify full-model performance, other split consumers, or complete policy removal.
