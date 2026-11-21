# Executed commands

All Cargo commands were run from the experiment directory, with `CARGO_HOME="$PWD/cargo-home"` and `CARGO_TARGET_DIR="$PWD/target"`; no shared Cargo cache or repository configuration was changed.

1. `cargo +1.99.1 fetch --locked` initially failed because the new experiment did not yet have a lock file. Cargo explicitly requested generating the lock file. No compilation occurred in that attempt.
2. `cargo +1.99.1 fetch` generated `Cargo.lock` and downloaded the exact pinned crates. Output: `fetch.log`.
3. `cargo +1.99.1 build --release --locked -j 2` built the dependencies and initial binary. Output: `build.log` (2m 07s on this host).
4. `cargo +1.99.1 run --release --locked -j 2` compiled and executed every behavioral scenario plus three small-shape repetitions. An initial successful run had a deprecated `Value::to_str` warning; the code now uses `as_str`.
5. `cargo +1.91.0 check --locked -j 2` checked compatibility with the installed older toolchain. Output: `rust-1.91-check.log`.
6. `cargo +1.99.1 fmt --all`, followed by the final `cargo +1.99.1 run --release --locked -j 2 > run.log 2> run.stderr.log`, formatted and executed the final source including version-identity checks.
7. `cargo +1.99.1 fmt --all -- --check` verified formatting.

`registry-verification.json` was fetched from `https://crates.io/api/v1/crates/automerge` and `https://crates.io/api/v1/crates/loro` with Python's standard-library urllib, recording latest stable version, selected version, yank status, published time, and declared Rust version. `environment.txt` captures `rustc +1.99.1 -Vv`, `uname -a`, and `lscpu`.

For an independent rerun, execute the two README commands. Crates are pinned by exact manifest versions and the lock file. Generated timings may vary, and ordering inside debug import-status maps need not be byte-stable; assertions compare semantic document state and causal heads/version vectors.
