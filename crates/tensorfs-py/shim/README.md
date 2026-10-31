# P0 Torch allocator adapter

Build the candidate wheel with `scripts/build-wheel.sh --torch-python <python-with-torch>`.
The interpreter supplies headers and libc10; it is not modified. The shim is built ahead of time,
and `tensorfs.varena.torch.install()` refuses a missing shim or an unlisted Torch/CUDA/C++ ABI.
The P0 builder supports Linux. Public release packaging and other platforms remain P1/P4 work.

In a fresh executor child, call `vt.install(device=0)` before Torch CUDA lazy initialization.
This selects the whole-process allocator; the old recovery hook cannot be installed alongside it.
P0 leaves the old entry point available for the current release until the replacement is qualified.

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

The supported Python statistics are `memory_allocated`, `memory_reserved`, their peak readers,
`reset_peak_memory_stats`, and the documented subset returned by `memory_stats`. They report
working allocations, including graph pools, separately from weight-region statistics. C++ readers,
snapshots/profilers, `MemPool`, CUDA IPC, cuDNN benchmark mode and Inductor graph trees are not supported
by this P0 adapter. Rebinding does not repair aliases imported before installation.

## Reviewed Torch ABI cohorts

One binary admits exactly `2.14.0+cu130` and `2.14.1+cu130`, CUDA 13.0, with matching C++ ABI and
platform. Metadata keeps the actual build's Torch version/commit as well as this finite list;
future patches require another review. A model keeps its locked Torch version.

The installed builds report commits `08187d9e0fba026dc8217405802ab5381dc88d90` and
`5c4886908584029761b579af026dcfb627c84070`. Their complete 11-header `Exception.h` dependency
closure, allocator headers, `libc10.so`, `libc10_cuda.so`, and Python memory/graph adapters are
byte-identical. All six c10 symbols referenced by the shim are present. The actual commits also
have identical [allocator callback code](https://github.com/pytorch/pytorch/blob/08187d9e0fba026dc8217405802ab5381dc88d90/torch/csrc/cuda/CUDAPluggableAllocator.cpp)
and capture/lifetime implementations. `libtorch_cuda.so` differs; this is not a numerical-parity
claim between Torch versions.

This establishes candidate ABI admission, not GPU qualification. At the 2026-10-31 compatibility
change, existing GPU receipts covered 2.14.1; the actual 2.14.0 stress/model runs remained pending
in tracker xs-138. Build and runtime hashes remain separate evidence.
