# Process-local CUDA weights

`tensorfs.varena` owns stable virtual addresses and reclaimable physical weight pages through
Rust. TensorFS remains the byte reader. A `Use.tensor` is an ordinary `torch.uint8` CUDA tensor;
its DLPack storage owns a native physical pin until its last alias dies. Runtime must fill all
logical bytes before `Use.commit()`. Commit joins the primary context before publishing ready
contents. An abandoned fill cannot be overwritten while an escaped tensor still pins it.

```python
from tensorfs import varena
varena.install_allocator()  # after importing Torch, before it reserves CUDA memory
arena = varena.Arena(0)
region = arena.reserve(byte_count)
use = region.prepare()
if use.needs_fill:
    checkpoint.components[component].tensors.read_into(key, out=use.tensor)
    use.commit()
# Keep use.tensor or a derived view alive for every kernel or graph that uses these bytes.
```

The compiled bridge ships inside `tensorfs._ext`; it has no Torch C++ dependency and does not
compile anything while serving. The Linux implementation changes verified `cudaMalloc` and
`cudaFree` data-import slots in loaded `libc10_cuda.so` and `libtorch_cuda.so`. The Windows x86_64
implementation uses the corresponding PE import tables in `c10_cuda.dll` and `torch_cuda.dll`.
It validates module layouts and original function pointers, temporarily enables writes on those
data pages, and restores their permissions. It does not change executable instructions.
Installation retains both original CUDA libraries and the hook extension for process lifetime.
It must precede concurrent CUDA work and other allocator/profiler hook installation.

Torch's native caching allocator still owns activation segments, splitting, caching, graph
pools and stream records. Every thread using the verified imports reaches the same native hook.
On a real CUDA allocation OOM, the hook reclaims inactive groups in its process/device, then
retries that one `cudaMalloc` call once if pages were released. Native errors return through
Torch's normal CUDA exception path. Direct native calls preserve CUDA codes on
`torch.AcceleratorError`; an unusable arena raises `PoisonedError`, whose `sticky` attribute is true, for executor
retirement, without inventing a driver error code. It never enters Python, acquires the GIL, reads bytes,
contacts another process, or reruns package operations. Reclamation may join the local primary
CUDA context; it has no bounded latency guarantee for opaque kernels.

`set_activation_limit(device, bytes)` optionally checks new hooked Torch segments plus current
resident weight pages. A deficit reclaims inactive weight groups before attempting the real
allocation. This is an **activation admission limit**, not a hard GPU/process cap: later weight
fills, CUDA contexts, and allocations outside the hooked imports can exceed it. Configure it
before Torch reserves memory. `allocator_stats()` reports actual hooked reservations, calls,
reclaims, refusals and retries; it is not a GPU ownership authority.

`capture(device)` forbids residency changes and allocation recovery across the entire process
on that device. Prepare and pin every dependency before capture/replay, and retain those pins
for graph lifetime. Raw Torch/CUDA captures outside Runtime must also enter this guard; the
streamless cudaMalloc hook does not infer arbitrary third-party captures. Reclamation synchronizes the primary context before unmapping, including
work on unknown side streams in that context. Private foreign CUDA contexts and IPC are not
supported.

Coverage excludes `cudaMallocAsync`, expandable segments, other CUDA libraries, static or
unknown Torch linkage, Torch modules loaded after installation, and runtime allocator changes.
Unsupported layouts/backends fail explicitly. Linux x86_64 Torch 2.14.1+cu130 is GPU-proven by
the stock-memory/native release-gate cell. Linux aarch64 and Windows x86_64 have implementation
only; the Windows hook has been cross-typechecked, not run on a Windows GPU. Neither is claimed
qualified until the same cell runs there. macOS and Windows ARM CUDA hooks are unsupported.

Implementation references: PyTorch documents that [`use_mem_pool`](https://docs.pytorch.org/docs/stable/cuda.html#torch.cuda.use_mem_pool)
routes only the current thread, so it cannot provide this process-wide allocation path.
The loader bindings follow [Linux dlopen](https://man7.org/linux/man-pages/man3/dlopen.3.html),
[Microsoft PE/COFF](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format),
[VirtualQuery's memory layout](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-memory_basic_information),
and [GetModuleHandleEx module pinning](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulehandleexa).
No GPL allocator implementation or source was used.

## Owned native refill

On supported Unix CUDA builds, `Region.prepare_read(checkpoint, parts)` admits the
region and initializes the existing TensorFS bounded staging rings and copy streams.
Each part is `(component/key#role, byte_offset, exact_stored_length)`. Offsets must
be nonoverlapping and inside the region; TensorFS fills every gap with zeroes.
The region belongs to this immutable recipe. Changing source or transform identity
requires a different region.

`supports_prefetch()` requires non-reclaiming admission, the native producer/capture guard,
and a separate replay recovery guard. Pass `reclaim=False`
for speculative work: a streaming-window deficit, physical allocation OOM, or an
abandoned mapped fill refuses admission without eviction, watermark reduction,
or a context join. Demand preparation keeps its existing recovery behavior.

Preparation starts no payload read. Call `start()` after active resident work has
been admitted. The native worker owns its checkpoint, local source, part descriptors
and Fill. It uses the existing buffered/O_DIRECT staging engine, with no Python
transformer, credential callback or DLPack-owned destination on the worker.
Staging/streams are warmed by preparation; timing events can still be acquired by
the read engine on first use. This is not complete CUDA-resource preallocation.
`poll()` and `wait()` report issued bytes separately from checked completion.
Only successful checked completion publishes the region; ordinary `prepare()` and
DLPack views can then consume it on any stream. This first interface does not publish
pending GPU writes, so no consumer waits on an unrecorded or pooled event.

`cancel()` requests that new bounded pieces stop; it does not join a running read.
`wait()` (or completed `poll()`) reports the checked drain of submitted copies.
Cancellation before `start()` settles immediately and releases the Fill without
requiring a later start. Cancellation never publishes partial data. Ordinary input failures also require checked producer
drain before the Fill is released. If completion is uncertain, the actual Fill,
region and source owners are retained and the arena is poisoned against reuse.
The existing conservative context join for unknown/escaped consumer reclamation
is unchanged. This is producer readiness, not a substitute for consumer lifetime.

The first overlap contract is a premapped/admitted raw destination beside a resident
compute loop. Decoder/kernel preload and separate evidence are required before any
decode-overlap claim. There is no persistent recipe index or allocator-OOM writeback.

Each admitted native read also owns a device producer lease from before mapping/staging
preparation through checked completion. Actual CUDA capture refuses while any such
lease exists, including an unstarted read; cancel or complete it before capture.
Producer admission likewise refuses during capture. Cancel-before-start releases this
lease immediately; unknown completion retains it with the actual owners. Resident graph
replay uses `varena.replay(device)`: it permits independent producers while suppressing
allocation recovery during the host launch. Retained physical pins protect graph dependencies
through device completion. Non-reclaiming producer admission and checked producer publication
may overlap this replay guard; paths that join a context or reclaim backing remain excluded.
