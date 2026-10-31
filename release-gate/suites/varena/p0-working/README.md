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
GPU and per-case results. `case=<name>` selects one case; default `all` requires all ten. Candidate build/installation
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
