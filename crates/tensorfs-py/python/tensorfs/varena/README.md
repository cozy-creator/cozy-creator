# Varena Python API

`tensorfs.varena` is the public allocator: `Gpu`, `Region`, `Use`, `Allocation`, `Cache`, `Plan` and `Source`.
`tensorfs.varena.torch` sends Torch's allocations through it and adds `offload`.

```python
from tensorfs import varena
from tensorfs.varena import torch as vt

vt.install(0)  # before any Torch CUDA initialization
```

Low-level calls take the actual CUDA stream your work runs on:

- `gpu.use(regions, stream)`: every other consuming stream joins this one before the Use closes.
- `gpu.alloc(bytes, stream)`: `allocation.record_stream` declares another consumer before it closes.
- `cache.release(offset, bytes, stream)`: every consumer of the range is ordered before this stream's completion.

Use context managers or checked `close()`. A refused close keeps the handle to retry. A finalizer hands ownership to the native
retirer without proving completion; `retirement_stats()` shows what is deferred or quarantined. Before destroying a Torch
stream, stop submitting and call `vt.retire_stream(stream)`.

See [the public contract](../../../../varena/PUBLIC-API.md) and [Torch offload](../../../../varena/TORCH-OFFLOAD.md), and the
[Region](../../../../varena/examples/python_client.py) and [Cache](../../../../varena/examples/cache_application.py) examples.

`tensorfs.varena._runtime` is private: the Runtime's source binding, weight pins, load continuations and cache owners.
