# Public ABI across bindings

Proves C, Rust and Python load in one process, in either order, and share one core: one mapped `libvarena.so` and one error state per thread. With a GPU,
C, Rust and Torch allocations also balance on one set of stats. [Contract](../../../../crates/varena/PUBLIC-API.md).

```sh
bash release-gate/suites/varena/public-api/prove-host.sh OUT    # no CUDA: builds the SDK, runs both load orders
```

On a GPU, with a matching Torch shim:

```sh
python mixed.py --c OUT/mixed-c.so --rust target/debug/libvarena_mixed_client.so --order native-first --gpu
python mixed.py --c OUT/mixed-c.so --rust target/debug/libvarena_mixed_client.so --order python-first --gpu
```
