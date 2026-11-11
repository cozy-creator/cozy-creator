# varena programs

varena's own tests, run as plain programs on the gate's rental by `release-gate/programs.sh` (xs-174). They test the
library, so they aren't packages: the package contract keeps packages off varena. Each uses only its public API
(`tensorfs.varena`, `tensorfs.varena.torch`).

| program | what it proves | was |
|---|---|---|
| `varena_offload.py` (runs `offload_adapter.py`, `offload_sdxl.py`; `--env diffusers`) | vt.offload's promises on tiny real models (a dropped handle, ties, direct reads and aliases, an escaping alias, the VAE's dtype flips, writes, streams, a block too big to pin, a loop's graph, a fork), and SDXL's two full calls stock, eager and graphs, with times, NVML peak and host memory (`crates/varena/TORCH-OFFLOAD.md`) | `varena/torch-offload` |
| `varena_core.py` (runs `core.py`) | the memory authority's rules under `vt`'s allocator: the cap counts the driver, the fast path's cost against stock Torch, bounded fences, a wait stalls nobody, a split's sample is live, freed pages back any block, pool use leaves the pass measured | `varena/core` |

Findings: cases of `varena/core` that exist only through the Runtime's private adapter (`tensorfs.varena._runtime`,
`tensorfs._ext`), dropped from the program for the coordinator to decide whether they become Runtime built-ins:
- abandoned-fill, pending-working-before-weights, split-waits-and-caps, fill-keeps-phase-pages, eviction-order,
  prefetch-rule, fill-paths-agree: weights Regions built through `_runtime.Device.region(bytes, Source(Pages, name,
  offset, bytes, derive=...))` and `_runtime.Plan(device, phases)`. The public `Gpu.region(Source | bytes)` and `Plan` take
  different sources (no staging `Pages`, no `derive` callback) and no phase list, so these cases have no public form.
- owner-wait-physical, owner-wait-capped, short-holders-wait: `_runtime.CacheOwner` (a cache that shrinks on request)
  and `_runtime.request_cache_shrink`; short-holders-wait also `_ext._VarenaNoCapture`. No public cache-shrink request.
- capture-refuses-a-fill: `_ext._VarenaAllocatorCapture`, the allocator's capture guard. No public capture state.

Findings from `varena/torch-offload`:
- `OffloadError` lives only in the private `tensorfs.varena._offload`; `offload_adapter.py` catches its public base,
  `RuntimeError`.
- The check that a managed weight became `_offload._Weight` is an implementation detail: dropped.
- `in_executor` proved vt.offload inside a Runtime executor, on its allocator and stream. A program is no executor, so
  that is the Runtime's to prove (a built-in), not varena's: dropped here.
