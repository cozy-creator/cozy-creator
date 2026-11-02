# Varena Python API

`tensorfs.varena` exposes one allocator interface: `Gpu`, `Region`, working
`Allocation`, `Cache`, and `Plan`. Its types are the same objects as
`tensorfs.varena.api`; the root exports no old Arena, Cache or cooperation aliases.
All bindings resolve the same native core linked by `tensorfs._ext`.

```python
from tensorfs import varena
from tensorfs.varena import torch as vt

vt.install(0)  # before any Torch CUDA initialization
```

The thin Torch adapter sends every working allocation to Varena. Torch retains no
second allocation cache. The wheel carries the adapter built for its declared
Torch/CUDA ABI; nothing compiles while serving.

Low-level consumers must name their **actual** CUDA stream:

- `gpu.use(regions, stream)` retains weight uses; all other consuming streams must
  join that stream before the use closes.
- `gpu.alloc(bytes, stream)` creates working storage. `allocation.record_stream`
  declares another actual consumer before the allocation closes.
- `cache.release(offset, bytes, stream)` releases a page-aligned range only after
  every consumer of that range is ordered before that stream's completion.

No omitted stream silently selects zero. A framework adapter honors the stream
where the framework submitted work. Standalone examples own nonblocking CUDA
streams and use their own completion events. Retain a borrowed stream until its
checked and deferred Varena owners have retired; stop submissions and call
`vt.retire_stream(stream)` before destroying an external Torch stream.

Use context managers or checked `close()`. A refused close keeps the handle so
the caller can retry after the actual exclusion/completion boundary. A finalizer
transfers ownership to native retirement; it does not prove completion or free
unproven backing. `retirement_stats()` exposes deferred and quarantined work.

See [the public contract](../../../../varena/PUBLIC-API.md) and runnable
[Python region example](../../../../varena/examples/python_client.py) and
[cache example](../../../../varena/examples/cache_application.py). The cache
example's explicit shrink request is labeled private qualification instrumentation.
General `torch.offload(pipe)` remains unimplemented.

## Private Runtime integration

`tensorfs.varena._runtime` supplies TensorFS/Runtime source binding, DLPack weight
pins, prepared load/graph continuations, phase decoration and weak owner callbacks.
Those consumer contracts are not a second public allocation API and have no root
reexports. `_cooperative` is the internal Linux service adapter. Runtime imports
these modules explicitly; missing current native members are cohort errors.
Linux-only cooperation and the optional native store feature remain actual platform
and build boundaries. Old release signatures and root aliases are not supported.

The existing private TensorD Room path remains for its unmigrated lanes; moving
its Python import does not claim that policy is retired. All advanced source, graph
and lifetime qualification remains tied to its recorded native/Runtime artifacts.
