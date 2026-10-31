# P0 Torch allocator adapter

Build the candidate wheel with `scripts/build-wheel.sh --torch-python <python-with-torch>`.
The interpreter supplies headers and libc10; it is not modified. The shim is built ahead of time,
and `tensorfs.varena.torch.install()` refuses a missing shim or mismatched Torch/C++ ABI.
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

The supported Python statistics are `memory_allocated`, `memory_reserved`, their peak readers,
`reset_peak_memory_stats`, and the documented subset returned by `memory_stats`. They report
working allocations, including graph pools, separately from weight-region statistics. C++ readers,
snapshots/profilers, `MemPool`, CUDA IPC, cuDNN benchmark mode and Inductor graph trees are not supported
by this P0 adapter. Rebinding does not repair aliases imported before installation.
