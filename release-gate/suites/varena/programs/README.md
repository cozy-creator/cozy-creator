# varena programs

varena's own tests, run as plain programs on the gate's rental by `release-gate/programs.sh` (xs-174). They test the
library, so they aren't packages: the package contract keeps packages off varena. Each uses only its public API
(`tensorfs.varena`, `tensorfs.varena.torch`).

| program | what it proves | was |
|---|---|---|
| `varena_offload.py` (runs `offload_adapter.py`, `offload_sdxl.py`; `--env diffusers`) | vt.offload's promises on tiny real models (a dropped handle, ties, direct reads and aliases, an escaping alias, the VAE's dtype flips, writes, streams, a block too big to pin, a loop's graph, a fork), and SDXL's two full calls stock, eager and graphs, with times, NVML peak and host memory (`crates/varena/TORCH-OFFLOAD.md`) | `varena/torch-offload` |
| `varena_core.py` (runs `core.py`) | the memory authority's rules under `vt`'s allocator: the cap counts the driver, the fast path's cost against stock Torch, bounded fences, a wait stalls nobody, a split's sample is live, freed pages back any block, pool use leaves the pass measured | `varena/core` |

The cases of `varena/core` that only the Runtime's own varena adapter reaches (`tensorfs.varena._runtime`, `_ext`) test
the Runtime's integration with varena, so they are Runtime built-ins, `bench.varena_core` (`cozy_runtime/internal/bench/
varena_cases.py`), which `../drive` calls as cells `core-<case>`: abandoned-fill, pending-working-before-weights,
split-waits-and-caps, fill-keeps-phase-pages, eviction-order, prefetch-rule and fill-paths-agree (weights Regions with
staging `Pages` and a `derive`, phased `Plan`s); owner-wait-physical, owner-wait-capped and short-holders-wait
(`CacheOwner`, `request_cache_shrink`, `_VarenaNoCapture`); capture-refuses-a-fill (`_VarenaAllocatorCapture`).

Findings from `varena/torch-offload`:
- `OffloadError` lives only in the private `tensorfs.varena._offload`; `offload_adapter.py` catches its public base,
  `RuntimeError`.
- The check that a managed weight became `_offload._Weight` is an implementation detail: dropped.
- `in_executor` proves vt.offload inside a Runtime executor, on its allocator and stream. A program is no executor, so
  it is the Runtime's built-in `bench.offload_in_executor`, which `../drive` calls as cell `offload-in-executor`.
