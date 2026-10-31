# Varena v2 P0 memory and capture proof

This bounded, model-free CUDA application runs through the ordinary Cozy CLI. Its fresh child installs the
candidate whole-process allocator before CUDA initialization. It downloads no weights and creates at most
256 MiB of fixture-managed working storage; CUDA contexts and library overhead are reported separately.

On an owned machine carrying the candidate TensorFS wheel:

```sh
cozy machine update "$NAME" --tensorfs "$TENSORFS_WHEEL" -y
cozy run ./release-gate/suites/varena/p0-working/gpu --on "$NAME" --gpus=1 \
  tensorfs_version="$TENSORFS_VERSION" extension_sha256="$EXTENSION_SHA256" \
  shim_sha256="$SHIM_SHA256" seed:=138
```

Get expected version and the two binary hashes from the exact candidate wheel, before installing it. The fixture
checks the installed extension (which contains Rust Varena) and shim, then records paths, Python, Torch commit,
GPU and per-case results. `case=<name>` selects one case; default `all` requires every registered case. Candidate build/installation
identity is separate from a source checkout. Never install this candidate into a shared local generation.

Cases cover empty capture initialization, 512-byte alignment, same-stream reuse, cross-stream retirement, reuse
inside capture, retained outputs after graph destruction, changed replay stream, shared and distinct graph pools,
clean capture exhaustion, and three graph replays after managed weight-region eviction/refill at the same address.
Driver memory/event counters are compared inside capture, outside setup and teardown. Each physical-byte
partition must balance; after every case all fixture allocations, graph pools and allocator event handles retire.
Cleanup seals deferred frees, synchronizes their completion and trims again. The `default-deferred-free` case
checks metadata-only default-stream reuse, then switches streams and verifies pending memory is not reused early.
The RNG case starts with no retained eager blocks, compares default and custom generators with eager byte-exact
references across two replays, reseeds and reproduces the first result, then verifies retained-output and final
teardown lifetimes. Its auxiliary default-stream allocations must also use pre-mapped capacity. No mock allocator/driver,
whole-card ballast, injected device fault, or package-call retry is used. Full SDXL/H3 performance/quality and real
card-pressure qualification remain separate P0 requirements.

## Allocator microbenchmark

Use the same exact candidate binary version/hashes as the stress call:

```sh
cozy run ./release-gate/suites/varena/p0-working/benchmark --on "$NAME" --gpus=1 \
  tensorfs_version="$TENSORFS_VERSION" extension_sha256="$EXTENSION_SHA256" \
  shim_sha256="$SHIM_SHA256" samples:=7 rounds:=50000
```

The app launches a fresh Torch-native child, waits for it to exit, then launches a fresh Varena child with the
same Python executable and visible GPU. Both verify the candidate extension/shim hashes even though only the
Varena child installs that allocator. Each warms once, then measures seven identical samples of eight allocation
sizes (512 bytes through 8 MiB) on the legacy default stream. There is at most one live request at a time; mapped
fixture storage is checked against 256 MiB after warmup and every sample. Context/library memory is outside this cap.

The primary timing is host time for Python dispatch into Torch's C++ raw alloc/free entrypoints. No synchronization,
telemetry, kernel launch or copy occurs inside the pair loop. Setup and warmup are excluded; the completion-inclusive
sample duration and final drain are separately reported. Every sample, size, pair count, range, binary identity and
GPU identity is retained. The P0 microbenchmark passes when the median host nanoseconds per pair is no more than
2× native. It is not a C++-only measurement or a model-latency result; Python dispatch overhead remains in both arms.
The benchmark returns `Report(passed=false, detail=<full JSON>)` on a ratio failure so every failed sample survives
in the result artifact. A successfully delivered Cozy call is not qualification: inspect `result.passed` and the
unchanged 2× ratio limit. Child execution/provenance failures still fail the call.

Default and Torch-pooled streams live for the process. An owner of a destroyable stream must stop submissions,
synchronize after its final graph/tensor teardown, call `vt.retire_stream(stream)`, then destroy the handle. CUDA
stream destruction alone can return with work pending. Arbitrary external destruction without this retirement
contract remains unsupported; the default-stream benchmark does not qualify it.

The `replay-hook-failure` case launches real graph work on a different replay stream, then throws from Torch’s
per-graph replay-end hook. It checks that the original exception survives and the pool completion fence is still
recorded. A second arm resets the graph and requests trim inside that hook, requiring transient replay ownership
to retain the pool through completion recording. Both arms copy and validate the GPU result, then verify retirement.

`dlpack-record-stream` covers an interior-pointer DLPack view of working storage and a foreign TensorFS Region
alias. `external-stream-retirement` creates actual CUDA runtime streams, drains and retires each, destroys it,
then frees a retained tensor. Its three arms cover no remaining consumer, an already-declared consumer and a new
consumer declared after retirement. Both cases validate GPU output and final allocation/event retirement.

`pool-variable-reuse` fixes the prepared pool at 16 MiB. One arm frees a 16 MiB temporary then allocates 8 MiB;
the other frees two simultaneously live 8 MiB temporaries then allocates 16 MiB. Both measure a 16 MiB live
peak, copy into eager-owned destinations, check output bytes and make no allocator driver/event calls inside
capture. Each arm records its own result, including OOM, and neither retries with a larger pool. Both must pass.

## Fork rejection before CUDA

The separate CPU-only `/fork` entrypoint uses the same candidate version/hash payload. Its fresh child calls
`vt.install()` without opening a device, then makes six actual forks to check explicit status, empty-registry
stats, null alloc/free/record-stream callbacks and empty-registry trim. Each must reject the inherited process
before CUDA initialization. Separate forks prevent an earlier error latch from masking a later missing check;
the parent must remain usable with zero allocator/driver counters. This does not replace any GPU case or benchmark.

## Torch 2.14.0 production cohort

The fixture's project and lock select SDXL's exact Linux `torch==2.14.0` PyPI distribution. The CPython3.12
x86_64 wheel URL and SHA256 match `packages/sdxl/uv.lock` (SHA256 starts `fecffb58f51fd643`).
The original fourteen case bodies and allocator timing loop are unchanged; variable-size pool reuse adds one
case with two required arms. Both entrypoints default their expected
`torch_version` to `2.14.0+cu130` and fail before execution on another build. Receipts also record distribution
version, Torch source commit, CUDA version and C++ ABI; compare these with the actual model receipt because the
model's PyPI distribution may spell its version `2.14.0` while `torch.__version__` includes `+cu130`.

`cozy machine update` still selects the candidate Runtime/TensorFS pair. The fixture verifies their installed
extension and shim hashes. Do not replace that path with an untracked interpreter or install a different Torch
into an active generation. Preserve existing Torch2.14.1 receipts as evidence for their own cohort.
