# Zero-span export correction: host boundary

Root authorized this narrow correction after the read-only review of product `0af634eff39c1a28ea587e1b69516f9dc326d142` and fixture `3c678dfcd99a2b411ec0dd70d973f4a8075c15a8`. New branch `codex/xs138-zero-span-export` starts from fetched `origin/master` `a0338b898060fcd16f8bb169a638afc89f08f080`, with exact frozen context imported as `c9f53584247193dc9be5d7c3645887fdc553a347`. Only commits after that context checkpoint are adoption candidates. Original #2450 and the coordinator worktree stay frozen.

The production delta validates the original requested dtype/alignment/offset/span and logical bounds first, then chooses the existing Region base DataPtr only when span is zero. Nonempty pointer arithmetic and native consumer padding rejection are unchanged. The same `TensorExport` remains in the DLPack storage holder, preserving its full native Pin and Source/service custody.

`zero_span_cpu.py` exercises ten real Torch CPU DLPack capsules over actual ctypes allocations. It supplies logical-end controls and valid-base empty exports independently; it does not simulate a Pin, reproduce the production selection algorithm, or invoke the CUDA bridge. Empty tensors expose a zero tensor pointer but preserve the supplied Storage pointer. Detached/as-strided aliases retain the DLPack manager until their final alias disappears, with exactly one deleter call. Real nonempty interior strided/transpose/scalar cases retain their exact values and pointers. CUDA remains uninitialized.

The maintained `parts_gpu.py` has additive actual-GPU coverage for a one-byte Source followed by a trailing empty Source at offset256. It checks normalized Storage DataPtr, original empty/nonempty bounds and alignment refusal, recording on a second actual stream, exact payload/gaps, early Region/other-export closure, retained backing through an escaped empty alias and final reclamation without quarantine. This client is prepared but was not run by this lane. The coordinator owns GPU execution and installed-cohort qualification.

Native check/build artifacts use this task's `.task/cargo-home` and `.task/target`, with offline dependency resolution. No build uses the GPU operator's native target/cache or modifies installed cohorts. The real production extension host client tests metadata/refusal boundaries only; it does not qualify the production CUDA pointer choice or native GPU lifetime.

Existing source unit tests in `crates/varena/src/abi/host_parts.rs` and mocked packing resources in `torch-offload/packing.py` / `packing_metadata_cpu.py` remain a separately reported breach of the workspace's no-unit-tests/no-mocks rule. Those files and receipts are not changed or used as product qualification here. No policy, rental, merge or release is included.

Rerun the CPU consumer client with a real CPU Torch environment:

```sh
PYTHONDONTWRITEBYTECODE=1 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 \
  python release-gate/suites/varena/native-tensor-export/zero_span_cpu.py
```

Exact source/artifact hashes, native commands and bounded results are in `manifest.json`.
