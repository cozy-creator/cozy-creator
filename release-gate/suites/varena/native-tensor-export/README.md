# Native typed export clients

Files `cache-pages/prepare.py` and `torch-offload/prepare_packing.py` read.

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
