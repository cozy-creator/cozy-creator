# Native typed export / immutable Source clients

This lane provides the native interfaces for the separate Torch offload adapter (#2449); implementation draft #2450. It adds no model downloads, rentals, Python allocator policy or application OOM retries. GPU execution belongs to the coordinator.

## Host

Run the real CPU Torch Source client from a CPU Torch environment:

```sh
python release-gate/suites/varena/native-tensor-export/source_cpu.py
```

It checks whole-storage immutable snapshots, interior/transposed/broadcast views, all 15 admitted dtypes, dtype-derived versions and invalid metadata. It does not load CUDA or claim native storage ownership.

Build the actual extension with at most two workers:

```sh
nice -n 19 cargo check -p tensorfs-py --no-default-features -j 2
nice -n 19 cargo check -p tensorfs-py -j 2
nice -n 19 cargo build -p tensorfs-py -j 2
```

Point the native host client at that built extension, with Cargo's dependency directory and the toolchain runtime library directory on the dynamic loader search path. It loads production PyO3/core code directly, checks same-core identity and native metadata validation, and confirms export refusal without the Torch consumer hook installed:

```sh
TORCH_NATIVE_RUST_LIBDIR="$(rustc --print target-libdir)"
LD_LIBRARY_PATH="$PWD/target/debug/deps:$TORCH_NATIVE_RUST_LIBDIR" \
  python release-gate/suites/varena/native-tensor-export/native_host.py \
  --extension target/debug/lib_ext.so
```

This direct build check does not qualify installed wheel packaging or GPU behavior.

## Coordinator GPU client (prepared, not executed in this lane)

`gpu.py` is a standalone payload for the coordinator's ordinary Cozy release-gate invocation. Use a matched core, extension and Torch shim, installed before Torch CUDA initialization. Existing gate orchestration should run this client through Cozy's default home; the client does not rent or launch anything itself.

It checks typed interior and independent override-stream exports, escaped aliases after Use/Region closure, actual pending side-stream consumers with record_stream, final-alias release plus trim, immutable host/dtype versions, invalid native layouts and scalar/empty/broadcast exports. `--device` selects the actual device and `--delay-cycles` controls the explicit pending-consumer proof. An already completed CUDA event refuses that pending case and requires a longer delay; elapsed time alone never counts as a pass.

Additional coordinator gates remain: deterministically in-flight/failing foreign Sources, capture/custom access, actual offload adapter identity/mutation behavior, SDXL VAE dtype cycles and real model quality/performance. No host result is evidence for those gates.

`failed_tail_gpu.py` is a separate final-child regression for the coordinator's
checked retirement correction. It drops the last typed storage alias during an
actual CUDA graph capture, then verifies explicit quarantine and continued weight
accounting after ordinary trim. It intentionally retains that physical Pin and
Source/service custody until process exit; it is not a graceful-reclamation test.
The initial export, Source completion and capture resources are prepared before
capture. This fixture is authored, not GPU-qualified by the source commit.
