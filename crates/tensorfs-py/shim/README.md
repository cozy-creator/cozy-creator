# Torch allocator adapter

The shim links one Torch's C++ ABI, so a Linux wheel carries one `_varena_torch-<torch.__version__>.so`
per Torch our machines run: the list is `TORCH` in `backend/stage_shim.py`, and every wheel build
(`scripts/build-wheel.sh`, the release and dev workflows) builds each with uv. `install()` loads the
one for this exact `torch.__version__` and refuses any other Torch in one line naming the list.
It never compiles at runtime. Other platforms carry no shim.

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

## Adding a Torch

Add its exact `torch.__version__` (PyPI's Linux build) to `TORCH`; `cuda-toolkit[crt,cccl]` follows that
Torch's own CUDA pin. Version differences in Torch's headers are `TORCH_VERSION_MINOR` checks in
`varena_torch.cpp`. Drop a Torch once no image or machine runs it.

The private `varena_torch_workspace_cache_enabled` export reads
`at::cuda::isCUDABlasWorkspaceCachingEnabled()` through the existing contained-error
ABI. It reports Torch's actual latched setting without initializing CUDA or allocating
GPU memory; before 2.15 Torch always caches them per stream, and it reports 1. Managed graph frontends require this same-cohort export and refuse cached
library workspaces; low-level graph owners retain their own workspace lifetime contract.
