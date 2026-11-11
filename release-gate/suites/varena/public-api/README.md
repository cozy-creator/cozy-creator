# Public ABI across bindings

Proves C, Rust and Python load in one process, in either order, and share one core: one mapped `libvarena.so` and one error state per thread. With a GPU,
C, Rust and Torch allocations also balance on one set of stats. [Contract](../../../../crates/varena/PUBLIC-API.md).

```sh
bash release-gate/suites/varena/public-api/prove-host.sh OUT    # no CUDA: builds the SDK, runs both load orders
```

On a GPU, the gate's `public-*` rows: a plain program on the rental over `cozy machine ssh` (`release-gate/programs.sh`), not a
package, since the package contract keeps packages off varena (xs-174). Public API only: `tensorfs.varena`,
`tensorfs.varena.torch`, and the core's C ABI (`varena.h`) through ctypes where Python surfaces nothing.

```sh
bash release-gate/suites/varena/public-api/pack.sh OUT     # public_api.py and the clients' sources in OUT/tree
```

The pod builds the clients against its installed core, with Rust from rustup. Each case is a fresh process per client:

| `case` | Proves |
|---|---|
| `mixed` | `mixed.py --gpu` in both load orders: C, Rust and Torch balance on one set of stats and read one `room` |
| `engines` | `native_engine.c`; Rust `engine` with `room`, `split` and priority; `cache_application.py`'s holes, recommit and declined shrink; `python_client.py` |
| `sizing` | `sized_engine.c` under a 1 GiB cap: the level comes from `room` and `split`, and a measure replaces each estimate on the next pass |
| `priority` | `cells.py`: normal Regions go, the KV shrinks and a high Region stays; a declined shrink lets it go; with no high Region, weights go before the KV; a second process's ask follows the same order |

Findings (xs-174: what the Python public API doesn't surface, reached through the C ABI instead):
- `tensorfs.varena.Gpu` exposes no handle for a C client, so `mixed.py` opens the C client's own (`vr_v1_gpu_open`).
- Python has no `last_error`, core identity read beside the binding's, or room struct: `mixed.py` loads the core's C ABI
  (`tensorfs/.native/libvarena.so`, a path the public API doesn't name) with ctypes.
