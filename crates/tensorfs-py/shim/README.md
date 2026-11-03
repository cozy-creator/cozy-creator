# Torch allocator adapter

Build the candidate wheel with `scripts/build-wheel.sh --torch-python <python-with-torch>`.
The interpreter supplies headers and libc10; it is not modified. The shim is built ahead of time,
and `tensorfs.varena.torch.install()` refuses a missing shim or an unlisted Torch/CUDA/C++ ABI.
The builder supports Linux. Other platforms remain unqualified.

In a fresh executor child, call `vt.install(device=0)` before Torch CUDA lazy initialization.
This selects the whole-process allocator. P1 removes the old cudaMalloc import hook and its entry
points; native TensorFS buffers use the same working allocator.

Use `vt.graph(g, pool=pool, stream=stream, reserve_bytes=n)` to prepare and capture, then
`vt.replay(g, stream=stream)`. The default reservation is the observed eager allocation peak,
which is only a starting size; explicit undersized pools fail without allocating inside capture.
Empty captures can use `reserve_bytes=0`. `record_pool_stream(pool, stream)` declares later output
consumers. Graphs sharing a pool must follow their declared serial order, and output aliases retain
their own allocation ownership. Raw Torch capture/replay requires the same explicit completion hooks;
ordinary allocator hooks cannot observe replay launches.
Owners of native streams call `retire_stream(stream)` after draining submissions and before
destroying the CUDA stream. A freed tensor may outlive its original stream; the allocator must
not record new events on that destroyed handle.
Capture-thread auxiliary allocations, including Torch's lazy default-stream RNG seed/offset tensors,
also use the prepared pool. Other threads route only through Torch's capture filter. Nested captures
on the same device/thread are rejected; preparation cannot silently grow during capture.
If prepared capacity runs short, the shim throws a private `c10::Error` with the exact prefix
`[tensorfs.varena.capture_capacity.v1] `. cuDNN's workspace fallback catches `OutOfMemoryError`
and can cache a different engine; that is inappropriate for an undersized capture reservation.
`vt.graph` translates only this marked `RuntimeError` to public `torch.OutOfMemoryError` after
the actual capture ends and the failed graph is reset. Cleanup errors take precedence, and eager
physical OOM keeps its normal type. Unwrapped native capture owners receive the private refusal.

The supported Python statistics are `memory_allocated`, `memory_reserved`, their peak readers,
`reset_peak_memory_stats`, and the documented subset returned by `memory_stats`. They report
working allocations, including graph pools, separately from weight-region statistics. C++ readers,
snapshots/profilers, `MemPool`, CUDA IPC, cuDNN benchmark mode and Inductor graph trees are not supported
by this adapter. Rebinding does not repair aliases imported before installation.

## Reviewed Torch ABI profile

The Linux RC admits exactly `2.15.0+cu132`, CUDA 13.2, with the actual build's C++ ABI and platform. The installed SDK reports Torch commit `d0f2af8b66b20da4baa50644c0cd3f887a2eae2e`. Build metadata records that version/commit and the single supported profile; other builds require a new review.

The shim uses the SDK's `c10::MempoolId_t` directly. The actual CUDA pluggable-allocator header supplies the reviewed allocation/free, record-stream, reset and pool callback signatures; `c10::Error`, `OutOfMemoryError` and `AcceleratorError` remain the direct exception API. No older-Torch fallback is selected.

Header inspection, compile/link and host loading establish ABI-build evidence only. Actual CUDA allocator, capture/replay, lifetime and model execution remain separate integrated gates. Historical 2.14 qualification receipts remain preserved under their original source and artifact identities.
