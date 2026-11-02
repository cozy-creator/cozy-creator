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

## Deterministic Source and alias cells

`deterministic_gpu.py` is the primary prepared regression client for the coordinator. It replaces timed pending-work assumptions with the literal existing `public-source/source.c` provider and mapped HOST memory-value gates. The supplementary `gpu.py` and coordinator's original `failed_tail_gpu.py` are preserved.

| Child mode | Required actual observation |
| --- | --- |
| `ordering` | Producer and both initial/override consumers are pending; typed interior views read byte73 exactly. Escaped aliases retain native Pins after Use/Region close. Later interior consumers remain gated through final alias release and trim entering the third existing native fence wait; opening only that gate permits retirement while unrelated work remains pending. |
| `partial` | The existing provider really submits half the destination, returns SourceError, and yields no public Use/tensor. Job/Source custody persists until its producer gate opens and checked retirement completes. |
| `prepare_error` | The existing provider creates a stream but refuses prepare; no destination/Use/tensor is published. Its original checked Source cleanup retires that stream. |
| `capture_refusal` | Actual capture refuses a new typed export; the existing storage remains readable after capture and retires normally. |
| `failed_tail` | Dropping the final alias during capture quarantines the exact Pin/family/Source/service owner. Later trim returns with the unrelated gate still pending, resident bytes retained, and helper close still Busy. Quarantine persists until child exit. |

Each child admits one C Source. `source_provider.c` includes the existing provider directly, adding only observation locking, two controlled gates, primary-context custody and checked cleanup. `provider.py` supplies no Python callback, staging, allocator policy or retry. It retains helper code and partial/quarantined owners until checked close or process exit. The worker deadline is an explicit proof protocol; elapsed time supplies no completion evidence.

Build the helper in the same portable build environment as the other release-gate clients, linked to the exact core/Rust std cohort:

```sh
nice -n 19 cc -std=c11 -Wall -Wextra -Werror -fPIC -shared \
  -I crates/varena/include \
  release-gate/suites/varena/native-tensor-export/source_provider.c \
  -L NATIVE_LIB_DIR -Wl,-rpath-link,RUST_LIB_DIR -Wl,-z,defs \
  -lvarena -ldl -pthread -o varena-tensor-export-provider.so
```

The existing `cache-pages/freeze.py` archives the clean source checkpoint. Pass the new optional `--tensor-export-provider PATH` to `cache-pages/prepare.py` with its existing exact native helper/cohort arguments. Preparation verifies ELF/source/core hashes and the existing GLIBC floor, freezes both Python files from that archive, and emits `varena-tensor-export-provider.so-input-template.json`. A local host build is not a portable prepared artifact.

The existing controller registers `tensor_export_ordering`, `tensor_export_partial`, `tensor_export_prepare_error`, `tensor_export_capture_refusal`, and `tensor_export_failed_tail`. They reuse its existing native runner's PID birth checks, pidfd-owned cancellation/deadline cleanup and child-exit receipts. The new script mode additionally verifies the interpreter, script, helper and installed extension/core identities. Other helper modes keep their existing command and proof checks.

No GPU cell was executed by this lane. The coordinator must build/install the matching native observer bridge and run all five cells through the usual Cozy default-home harness. Host-staging changes remain with their assigned owner.
