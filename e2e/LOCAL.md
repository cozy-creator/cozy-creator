# Local browser feedback

The vault browser fixture can run on Linux against a dedicated local Calcifer. The runner owns a separate home, port, browser context and temporary projects. Its persistent profile retains downloaded media tools and the pinned Playwright driver between runs.

```sh
python3 scripts/test-local-web.py
```

The first run builds Cozy and the embedded page. Later runs reuse `target/dev-tests`. Google Chrome, pnpm, the repository Rust toolchain and FUSE are required. On Linux the runner prepares the repository's pinned securefs provider once. A distribution binary can be supplied directly.

Development builds optimize Argon2 to keep passphrase operations responsive.

```sh
python3 scripts/test-local-web.py --binary /absolute/path/to/cozy
```

Each run uses an immutable copy of that binary inside the test profile, so a concurrent Watchexec rebuild cannot replace the CLI halfway through a test.

For UI iteration, Vite serves current sources and proxies to the test Calcifer. Watchexec queues the next run when the source or fixture changes.

```sh
python3 scripts/test-local-web.py --vite --watch
```

Pass `--target-dir target/dev-local` to reuse the Cargo cache used by `scripts/dev-local.sh`.

Use `--securefs /absolute/path/to/securefs` for an explicitly prepared helper. `scripts/prepare-securefs.py` downloads and verifies the repository's pinned archive. `--harness` can select another checkout's vault fixture when comparing an unchanged binary with a fixture correction.

Each run writes its result, binary digest, timings and logs under `target/local-web/results`. Cleanup asks the owned Calcifer to close its views and stop, then stops any TensorD supervisor started for that test profile. A process that cannot drain leaves its profile and diagnostics available for inspection.

Per-run temporary files use a short owner-only directory under `/tmp` so Chromium's Unix sockets fit their path limit. Successful runs remove it. Its location is recorded with failed or interrupted results for inspection.

This runner exercises the real Rust service and real Chromium vault flow. The cross-platform release jobs remain the qualification for installed Windows and macOS behavior. Other browser cases that execute functions also need an explicitly prepared local worker and matching Runtime environment.

The focused native HTTP and browser-client contract test also creates and removes its own profile. It can share the test build cache.

```sh
pnpm --dir web/app install --frozen-lockfile
CARGO_TARGET_DIR="$PWD/target/dev-tests" CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=1 \
COZY_PRESENCE_CLIENT_FIXTURE="$PWD/web/app/tests/project-presence.mjs" \
cargo test --locked -p calcifer --lib project::presence_tests::native_presence_orders_and_client_contract -- --exact --nocapture
```
