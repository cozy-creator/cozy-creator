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

Source implementation and GPU receipts are pending. No passing proof is claimed by this scaffold.
