# Scoped capture readiness

`Gpu.use(regions, actual_stream)` establishes producer ordering and retains physical Pins. It may return before the native producer has completed or retired its staging/resources. Region Ready is published before the producer guard is released. Before actual graph capture, call the additive public seam:

```c
vr_status vr_v1_use_wait_ready(vr_handle use);
```

```rust
Use::wait_ready(&self) -> Result<()>
```

```python
used.wait_ready()  # ctypes.CDLL releases the GIL
```

The call retains the canonical Use lease and serializes against concurrent close with its pins mutex. It snapshots each pinned Region's current-generation submission control, including Ready Pins without their own submission field. Success requires both `completed` and `retirement_finished`, followed by same-generation/control identity, contents and arena-health validation. Failure/refusal leaves the Use owned unless another caller already closed it before admission. Source, retirement, quarantine and local wait failures are errors, never successful readiness.

Each waiting invocation owns a platform `Signal`. Weak observers are registered under the existing facts mutex; completion/failure publishes wakeups there. The signal is cleared before facts are checked, preventing a transition from being lost between observation and blocking. Worker quarantine also wakes subscribers after an observer poison, while preserving the poison for readers. Only `platform::memory::Signal`, its `Waitable`, and `wait_any` are used for the new wait. No registry, capture, arena, submission or facts mutex is held across `wait_any`; the Use pins lease remains held so concurrent close cannot end its physical ownership.

Call outside capture/replay. The existing ABI retirement exclusion refuses active capture/replay and excludes new capture/replay during the call; it does not hold the capture mutex while sleeping. Native workers can complete and retire resources under their existing nested retirement tokens. This is not consumer completion, a global retirement drain, CUDA/context synchronization, or a promise that unrelated producers will permit subsequent capture. Actual capture admission stays atomic and separate.

For streamed graph definitions, retain immutable Sources/Versions, OPEN Regions/VA, Plan and graph/static Working owners, not permanent weight tensors. Pre-export capture bindings before `vt.graph`, wait ready, end capture/routing, then drop capture exports outside exclusion. Each replay reacquires/producer-orders its exact Regions before the host replay guard and uses transient ordinary exports for checked last-consumer fencing/quarantine. Exit the guard before output-copy allocations and Use close. Reset/rebind must serialize admission and preserve execution-local owners until actual consumer tails are installed. No unpinned ordinary tensor or Pin-detach operation is provided.

## Host/build commands

Set task-owned Cargo and target directories. Do not use the GPU operator's artifacts/cache.

```sh
CARGO_HOME="$private_cargo_home" CARGO_TARGET_DIR="$private_target" \
  nice -n 19 cargo check --locked --offline -p tensorfs-py --no-default-features -j 1
CARGO_HOME="$private_cargo_home" CARGO_TARGET_DIR="$private_target" \
  nice -n 19 cargo build --locked --offline -p tensorfs-py -j 1
VARENA_LIB_DIR="$private_target/debug/deps" CARGO_HOME="$private_cargo_home" \
  CARGO_TARGET_DIR="$private_target" \
  nice -n 19 cargo build --locked --offline -p varena-client --example parts -j 1
LD_LIBRARY_PATH="$private_target/debug/deps:$rust_lib_dir" \
  python release-gate/suites/varena/native-tensor-export/use_ready_host.py \
  --extension "$private_target/debug/lib_ext.so"
```

The host client calls the actual built C ABI with invalid handles and checks that CUDA remains unloaded. It is not a native-GPU wait, lifetime, or GIL-blocking qualification. The production Python wrapper's Invalid propagation is also checked on the actual built core in the receipts. No unit tests or native-resource mocks qualify this feature.

## Prepared coordinator GPU client

Build the existing real provider with its additive mutex controls against the matched core:

```sh
cc -std=c11 -Wall -Wextra -Werror -fPIC -shared \
  -I crates/varena/include \
  release-gate/suites/varena/native-tensor-export/source_provider.c \
  -L "$native_lib_dir" -Wl,-rpath,"$native_lib_dir" -lvarena -ldl -pthread \
  -o "$client_dir/use-ready-provider.so"
```

Inside the coordinator's ordinary bounded Cozy child, the payload command is:

```sh
python release-gate/suites/varena/native-tensor-export/use_ready_gpu.py \
  --provider "$client_dir/use-ready-provider.so" --device 0 --gpu-uuid "$gpu_uuid" \
  --mode retirement_pending --timeout-s 60
```

Run one isolated child per mode: `retirement_pending`, `unrelated_retirement`, `capture_refusal`, `concurrent_close`. Reuse the existing actual C Source: mapped HOST memory-value gates establish pending device work; a real Job-retirement mutex makes checked cleanup return Busy; the literal Source.borrowed mutex retains an unrelated Source. No completion, CUDA status, SDK function or allocator is replaced.

The first mode checks both an ordered Use and a second Use acquired after Ready publication while resource retirement is still held, exercising distinct native wait Signals on the same fill. The unrelated mode checks scoped host Source readiness while another Source's checked retirement and another CUDA stream remain pending. Capture/replay refusal preserves the original Use. Concurrent-close is bounded stress only: a Python thread-start marker does not prove native wait entry, and the report explicitly says so. No false deterministic-entry claim is made.

All four GPU modes are prepared and unrun in this lane. The coordinator owns matched installed-cohort preparation, whole-child deadlines/cancellation and execution. Source/retirement driver failures and full generic graph-offload behavior remain their own qualification gates. Current-master guard/build files and executor protocol are preserved in the context; feature commits add no unsafe Rust file and do not alter those guards or canonical branches. The complete root reconciliation/advisory guard is not claimed from an old native context build.
