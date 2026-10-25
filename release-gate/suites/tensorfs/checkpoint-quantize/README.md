# Native checkpoint quantization

Real default-CLI CPU cells, no mocks or unit tests:

```
cozy run /absolute/worktree/release-gate/suites/tensorfs/checkpoint-quantize/run mode=basic seed=209001 --on local --json
cozy run /absolute/worktree/release-gate/suites/tensorfs/checkpoint-quantize/run mode=cancel seed=209002 --on local --json
cozy run /absolute/worktree/release-gate/suites/tensorfs/checkpoint-quantize/run mode=cancel-write seed=209003 --on local --json
```

`basic` writes actual raw F32/F16/BF16 checkpoint tensors and inherited config/assets, runs the
native Writer selection at two buffer limits, compares payload/companion bytes exactly with
Runtime's production producers, checks saturation/max-error exactly and F64 SSE/energy at
2e-13 relative tolerance, verifies both NVFP4 passes in source read accounting, refuses invalid
bounds/shapes/duplicate selections before entries, and proves zero-I/O completed reuse.
A real relative-error tripwire leaves no current tensor entry. No alternate store/home is set.

`cancel` writes a real64MiB tensor after one small tensor, observes actual native read/completion
progress, refuses commit/abandon while busy, cancels the running conversion, destroys/reopens its
existing keyed Writer, refuses a changed recipe, resumes only missing work with a different chunk
limit, and verifies the final stored checkpoint. It refuses a missed cancellation window.

The fixture consumes the installed TensorFS0.15.4 candidate. Root coordinates SDK changes;
this fixture installs no SDK and rents nothing. Installed CPU results are recorded below. GPU and production Runtime-cutover proof remain separate.

`cancel-write` waits for actual bytes entering the existing Writer part reader, cancels inside
part publication, then retries using the same Writer. No partial tensor may become a completed
key; this covers both spool and grid ownership through the existing writer.

Installed CPU qualification passed2026-10-25: **17496 basic,17497 cancel,17498 cancel-write**,
TensorFS6431a5387/wheel0171ff04…/native850d976d…, Runtime d6ec142ad, TensorD9998cf5f4,
on host meliodas through the default CLI. Basic passed6 conversions at each of2buffer limits,
exact producer payload/parts and acceptance facts,110592logical sourcebytes (bothNVFP4passes),
21720encodedbytes, reuse/inheritance/refusal checks. Cancellation after99328sourcebytes preserved
the first tensor; part-write cancellation after1048836Writerbytes likewise left no partial tensor
entry. Reopened and same-Writer retries each wrote only the missing64MiB-source tensor and kept
reused quality summary unknown. [Exact receipt](evidence/6431a5387-cpu.json).
These are CPU Writer-lifetime/numerical results, not the Runtime caller or a GPU/model proof.
