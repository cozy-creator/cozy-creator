# Current NCCL2 allocator client

`host.py` exercises the actual built extension and C++ allocator through Runtime's CPU-only binding seam. It reads a genuinely empty snapshot before any device is opened, repeats the binding and verifies a rejected second native installation remains an ordinary error. It does not initialize CUDA or qualify physical notifications.

`nccl_gpu.py` is an operator-owned standalone public SDK proof, not a new Runtime controller mode or a full Runtime/model proof. It requires the final gap-corrected observer cohort, exact native/adapter file hashes and two or four distinct actual GPU UUIDs. Run it in a fresh child group on the authorized machine with the matching installed SDK and Torch2.15.0+cu132. The ordinary Cozy full H3 Ulysses2/4 runs remain separate required gates.

```bash
CUDA_VISIBLE_DEVICES="$SELECTED_UUIDS" "$CANDIDATE_PYTHON" -m torch.distributed.run \
  --standalone --nproc-per-node=2 nccl_gpu.py \
  --gpu-uuids "$SELECTED_UUIDS_JSON" \
  --extension-sha256 "$EXTENSION_SHA256" --core-sha256 "$CORE_SHA256" \
  --shim-sha256 "$SHIM_SHA256" --torch-adapter-sha256 "$ADAPTER_SHA256" \
  --output "$NEW_RECEIPT_DIRECTORY"
```

The four-rank cell changes only rank count and the actual four-device binding. The launcher must preserve child exit/stderr, require every rank's `passed=true`, and independently retain all rank PID/UUID facts. This source performs no rentals, installations, downloads or remote actions. All artifact/hardware bindings are UNBOUND until the operator selects them.

The client requires actual pre-group 768KiB backing with a nonzero native unassigned tail and full ordered snapshot coverage; current NCCL2 type; complete all-to-all readbacks; a late64MiB map; an actual Region export as a collective input after Use/Region handle close; and a real prepared graph pool, capture/replay, retained output and final retirement while the communicator remains live. Capture must make no native physical-memory driver calls. Group destruction must complete. No symmetric-window, benchmark or universal leak claim follows.

The original f90 cohort can establish the NCCL2 segment-registration path through ordinary H3, but it predates the inactive-gap snapshot correction. Do not weaken this client's complete-block-coverage assertion to fit f90. Callback-failure physical-retention GPU execution remains separately pending; source inspection alone does not certify that case.

The snapshot's inactive gap ranges describe actual mapped space with no native allocation slot. They grant no reusable bytes or admission credit. Native subscriber seeding is atomic; Torch's separate snapshot/trace-attach calls are not an atomic API handshake. The pre-existing-buffer control and actual Runtime group-before-model route bound this proof.

## Real callback-refusal follow-up (prepared, unrun)

`probe.cpp` installs a genuine C++ Torch trace listener. It counts only actual native notifications; no fixture calls the listener synthetically. Its armed, exact-address retirement refusal throws into the real shim containment boundary. `refusal_gpu.py` runs in a fresh one-GPU child, fills3MiB with byte77, closes the tensor, triggers actual trim, and requires both the observer refusal and complete direct CUDA readback of that retained mapping. The poisoned mapping intentionally lasts until process exit. Other segments may have been freed before the target refusal; no global zero-release claim is made.

The helper uses the same current Torch/CUDA headers and compiler ABI as the adapter:

```bash
c++ -O2 -std=c++20 -shared -fPIC -Wall -Wextra -Werror \
  -D_GLIBCXX_USE_CXX11_ABI=1 \
  -isystem "$TORCH/include" -isystem "$CUDA_RUNTIME_INCLUDE" -isystem "$CUDA_CRT_INCLUDE" \
  probe.cpp -L "$TORCH/lib" -L "$CUDA_RUNTIME_LIB" \
  -ltorch_cuda -lc10_cuda -lc10 -l:libcudart.so.13 -Wl,--no-undefined \
  -o "$NEW_PROBE_SO"
```

Keep the exact library loaded until process exit: Torch trace listeners do not detach. `trace_host.py --probe PATH --probe-sha256 SHA` checks actual listener registration without initializing CUDA. It does not exercise a GPU callback. The GPU refusal client additionally requires actual GPU UUID and extension/core/shim/adapter hashes; its `--help` lists the required fields. It belongs in the ordinary Cozy fresh-child wrapper with one GPU and the usual child-exit/inner-verdict checks, not inside a persistent model executor. The operator binds and executes it; this source lane has not run either new probe client.
