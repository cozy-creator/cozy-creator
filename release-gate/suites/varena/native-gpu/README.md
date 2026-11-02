# Native GPU helpers in the shared build

These targets reuse the actual async-fill, cache/Plan/retirement and ledger fixtures.
They depend on Varena alongside TensorFS and the mixed client. No second core is embedded,
and the fixture source is not copied into a parallel implementation.

Build one private Rust dependency cohort:

```sh
cargo build --locked -p tensorfs-py --features extension-module -p tensorfs \
  -p varena-mixed-client -p varena-native-gpu-proof --lib --bin tfs \
  --bin varena-async-fill --bin varena-cache-pages --bin varena-ledger-peer -j 1
```

Package these binaries with that exact dependency `deps/libvarena.so` and matching
Rust standard library. Use the installed wheel's shared library for mixed-client runs.
A separate primary `cargo build -p varena` can select a different private Rust linkage
mode and must not overwrite the dependency core after this build. The public Rust client
uses the C ABI and deliberately does not depend on the native implementation crate.

Building these helpers is not GPU qualification. The source fixtures retain their existing
GPU identity, real-byte readback, capture, failure and physical-accounting assertions.
