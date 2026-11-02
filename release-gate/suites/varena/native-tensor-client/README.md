# Actual typed tensor clients through Cozy

Two ordinary accelerator entrypoints run the unchanged `native-tensor-export`
clients, each in a fresh child owned by pidfd. The stdlib launcher checks wheel
metadata and files without importing Torch or TensorFS, then calls the original
script's `run` exactly as its CLI does. That function owns `vt.install` before
CUDA initialization. Parent and child validate the actual GPU UUID. After the
original function completes, the child verifies one mapped exact native core and
Rust std, the extension/shim and identical native core identity across the two
Python surfaces.

- `typed_exports`: all five exact original checks and zero final resident weights.
  The original stream retirement executes before wrapper success.
- `failed_tail`: actual capture-tail refusal preserves the original quarantine
  increment and retained weight bytes. This deliberately exits with unproven
  custody and makes **no physical reclamation claim**. It never shares a child
  with the normal export case.

All original assertions, numerical readbacks, synchronization and cleanup remain
in the byte-identical scripts. Full original results/stdout/stderr are retained
before a failed wrapper verdict. A canceled/deadline-exceeded invocation terminates
only its owned child. There is no additional warm-up, allocator bind or native
build. This is source preparation, not GPU qualification.

Prepare from a clean frozen worktree, using the actual paired wheels:

```sh
python3 release-gate/suites/varena/native-tensor-client/prepare.py \
  --source-root "$FIXTURE_ROOT" --source-sha "$FIXTURE_SHA" \
  --script-source-sha 6e22f6ea6f485707640a3fcaa3dd60fd1a1ac6d6 \
  --tensorfs-wheel "$TENSORFS_WHEEL" --runtime-wheel "$RUNTIME_WHEEL" \
  --gpu-uuid "$OBSERVED_GPU_UUID" --output "$NEW_OUTPUT"
```

The exact `--script-source-sha` is recorded by the source receipt; use its full
resolved SHA. Inputs are flat `Probe` fields. `UNBOUND` preparation is allowed,
but execution refuses it. Every distributed native duplicate must match Python's
canonical `.native` bytes. Current Calcifer-routed CLI files are retained when
merging the native source snapshot; no CLI/native product is built from this
fixture branch.
