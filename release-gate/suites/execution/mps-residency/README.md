# Real Torch MPS residency

The ordinary GPU CLI entrypoint uses the installed candidate Runtime/TensorFS and requires
an actual MPS device. It writes real F32/F16/empty checkpoint tensors, loads a64MiB+4096-byte
weight whose last element changes the output, and checks warm reuse, escaped alias retention,
cache release and refill. TensorFS reads bounded CPU windows; Torch owns MPS copies/storage.

```sh
cozy run /absolute/worktree/release-gate/suites/execution/mps-residency/gpu \
  seed=103 --on local --gpus=1 --json
```

No Varena/Metal allocator, mocked backend or private interpreter. This is a small correctness
cell, separate from actual BiRefNet/SDXL model smoke and from Windows/macOS shipping enablement.

[Current-library proof 40189558417](evidence/7b5-7d1-365bb-library.md) passed real residency
and BiRefNet output with Runtime 7d1 and TensorFS 0.15.8 through the explicitly historical
Mac-capable CLI 7b5. SDXL returned the permitted capacity refusal. This preserves current
CLI admission policy and does not establish local Mac shipping support.
