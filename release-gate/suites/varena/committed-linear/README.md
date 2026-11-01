# Explicit committed linear phase

This fixture uses the ordinary Cozy request and an actual local TensorFS checkpoint.
It does not opt H3 or any opaque loop into committed execution. The initial consumer is
one raw stored INT8/BF16 linear operation. Fill/bind callbacks, transformed weights,
multiple ranks and general Python compute callbacks are outside its supported contract.

`consumer` executes ten cases:

- Metadata inventory leaves source bytes, weight/pin state and every working allocator
  counter unchanged; the actual NativeLinear has Runtime's source pre/post hooks.
- An unknown current compute key declines raw prefetch and publishes after real completion.
- Identical compute with a now-established Region Plan reuses the successful key and
  completes the actual next raw source read. Source preparation is cold and outside compute.
- Replacing reservations while retaining the old addresses still reuses that compute key;
  both the source pointers and current Region-generation validation remain real.
- A changed input shape starts an unknown key.
- A native-derived tighter managed cap selects a smaller legal row choice and publishes
  a different current compute key; no Python capacity formula chooses the rows.
- An unrelated same-shaped CUDA weight refuses before working allocation/output writes.
- Actual native minimum refusal permits no output allocation, retry or compute sample.
- Closing the real checkpoint then forcing cold demand fails before compute and preserves
  the preceding successful sample.
- Different checkpoint contents with identical geometry start an unknown compute key.

Every successful consumer call is counted at the actual `EncodedLinear.forward` code
object using a Python profiler; it must execute once. Results are compared to the ordinary
native leaf using the existing provider error budget. Instrumented mechanism evidence is
separate from any uninstrumented performance claim. Inventory never probes by running the
package forward or creates a warm-up call to bind weights.

`protocol` covers successful ownership, changed-choice identity, abort preserving the
previous sample, reselection refusal, stale Region-generation rejection, undeclared
allocation, real CUDA capture refusal, independent graph replay during compute, and actual
callback-interval allocation. Graph replay remains allowed and both outputs must be correct;
its interference invalidates only this eager observation. No mocked driver or fabricated OOM
exception participates. All producer allocations use real Torch/Varena storage.

Both entrypoints preserve the full partial report and error trace on failure. They require
exact working/weight baseline restoration. `cleanup` is an ordinary later request in the
same PID and checks that the reported model/request keys have already retired.

## Preparation and execution

Root owns the portable TensorFS build, Runtime wheel, SDK installation and paid GPU work.
`prepare.py` derives strict versions and source/extension/shim hashes from those actual
wheels, then freezes this clean fixture. Supply the observed GPU UUID, or preserve an
explicit unbound template until a rental exists. The application entrypoints are
`consumer`, `protocol`, then `cleanup` with each report's PID/keys. Do not run this fixture
against the older a96 native API: the committed Execution binding is required.

Native source/binding compilation, Python syntax, Ruff, offline lock resolution and the
actual CPU file-backed source check pass. The CPU check reads 4,198,400 bytes, exercises
both real source hooks, raises the actual closed-checkpoint error, and leaves CUDA
uninitialized. GPU execution, measured prefetch, the tighter-capacity numerical comparison,
replay coexistence and same-PID cleanup remain pending. No full-model or performance claim
is made by this source checkpoint.


## Explicit Runtime startup boundary

Cold protocol run25544 overlapped the executor's primed background GEMM verifier. Its
phase and leaf excesses match the verifier's M3/K1024 FP32 input and BF16 bias sizes;
no allocation stack was captured, so attribution is source/counter evidence. Native
correctly invalidated the contaminated sample. The fixture now calls actual
`int8_linear.native` before its initial baseline, joining that existing primed opener
as the real consumer already does. The8MiB/4MiB operation vectors and all9 cases are
unchanged. This is a Runtime fixture preparation boundary, not a Varena API dependency
on GEMM. The original25544 receipt remains preserved.
