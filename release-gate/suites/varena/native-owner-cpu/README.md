# Native owner host-control stress

`prove.py --source REPO --target EXISTING_CARGO_TARGET --out NEW_OUTPUT_DIR`
compiles the exact native owner source in an output-only crate, appending the
visibility-local `lifecycle.rs` harness there. Product source contains no unit tests.
The receipt records all product source hashes and the harness hash.

This is a real native-thread/channel/ownership stress executable: 512 blocked
metadata callers cancelled by stop, admitted versus still-queued operations, Drop
while a worker cannot yet finish, and cancellation after an actual metadata panic.
It initializes no CUDA, substitutes no GPU implementation, and makes no GPU-owner
or donor-service qualification claim. The command timeout is a failing harness
guard, not an accepted allocator result.

Actual CPU Hello/Shutdown remains `../owner-cpu/hello.py`. It additionally records
Linux native task names so a hidden Rust thread cannot evade its CPU-parent check.
GPU owner parity, GIL independence, fork and capture-aware retirement remain real
release-gate runs through the ordinary Cozy CLI.
