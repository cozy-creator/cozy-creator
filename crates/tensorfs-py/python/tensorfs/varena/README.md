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
Torch's normal CUDA exception path. It never enters Python, acquires the GIL, reads bytes,
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
for graph lifetime. Reclamation synchronizes the primary context before unmapping, including
work on unknown side streams in that context. Private foreign CUDA contexts and IPC are not
supported.

Coverage excludes `cudaMallocAsync`, expandable segments, other CUDA libraries, static or
unknown Torch linkage, Torch modules loaded after installation, and runtime allocator changes.
Unsupported layouts/backends fail explicitly. Linux x86_64 Torch 2.14.1+cu130 is GPU-proven by
the stock-memory/native release-gate cell. Linux aarch64 and Windows x86_64 have implementation
only; the Windows hook has been cross-typechecked, not run on a Windows GPU. Neither is claimed
qualified until the same cell runs there. macOS and Windows ARM CUDA hooks are unsupported.
