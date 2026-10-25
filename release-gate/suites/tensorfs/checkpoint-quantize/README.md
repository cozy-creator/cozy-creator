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
this fixture installs no SDK and rents nothing. These gates are pending; no passing CPU/GPU
or production Runtime-cutover claim is made merely because source checks pass.

`cancel-write` waits for actual bytes entering the existing Writer part reader, cancels inside
part publication, then retries using the same Writer. No partial tensor may become a completed
key; this covers both spool and grid ownership through the existing writer.
