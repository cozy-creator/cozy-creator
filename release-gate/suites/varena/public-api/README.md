# Public ABI across bindings

Proves C, Rust and Python load in one process, in either order, and share one core: one mapped `libvarena.so` and one error state per thread. With a GPU,
C, Rust and Torch allocations also balance on one set of stats. [Contract](../../../../crates/varena/PUBLIC-API.md).

```sh
bash release-gate/suites/varena/public-api/prove-host.sh OUT    # no CUDA: builds the SDK, runs both load orders
```

On a GPU, through cozy, which the gate's `public-*` cells do:

```sh
bash release-gate/suites/varena/public-api/pack.sh OUT     # the suite as one package: local code travels as its own directory
cozy run OUT/public --on MACHINE --gpus 1 case=mixed seed:=1
```

The pod builds the clients against its installed core, with Rust from rustup. Each case is a fresh process per client:

| `case` | Proves |
|---|---|
| `mixed` | `mixed.py --gpu` in both load orders: C, Rust and Torch balance on one set of stats and read one `room` |
| `engines` | `native_engine.c`; Rust `engine` with `room`, `split` and priority; `cache_application.py`'s holes, recommit and declined shrink; `python_client.py` |
| `sizing` | `sized_engine.c` under a 1 GiB cap: the level comes from `room` and `split`, and a measure replaces each estimate on the next pass |
| `priority` | `cells.py`: normal Regions go, the KV shrinks and a high Region stays; a declined shrink lets it go; with no high Region, weights go before the KV; a second process's ask follows the same order |
