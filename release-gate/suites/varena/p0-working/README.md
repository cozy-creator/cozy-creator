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
