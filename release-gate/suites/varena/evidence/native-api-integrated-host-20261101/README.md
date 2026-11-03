# Integrated native/public API host proof

Source `6c4f0e53a` passes the real host suite with one CPU/job and Rust1.99.1. Both load orders map one core, agree across five identities, share thread-local errors within a thread, and isolate errors between threads. C11/C++17 compilation/linking and standalone C/Rust host examples pass.

This combines ledger065, committed executionb6 and P4bf/5472; older integration snapshotfcaa was not reapplied. The two native merge resolutions preserve pressure suppression/committed exclusion and Ready-cache selection accounting. The runtime build target and SDK remain in the recorded durable output directory.

This is not GPU, portable-wheel, release or public-API-completion proof. Known P4 close/retry, private Rust compatibility and bundling findings remain open; shared service adoption is separate.
