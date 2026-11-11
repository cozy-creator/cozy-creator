# Local browser feedback

The vault browser fixture can run on Linux against a dedicated local Calcifer. The runner owns a separate home, port, browser context and temporary projects. Its persistent profile retains downloaded media tools and the pinned Playwright driver between runs.

```sh
python3 scripts/test-local-web.py
```

The first run builds Cozy and the embedded page. Later runs reuse `target/dev-tests`. Google Chrome, pnpm, the repository Rust toolchain and FUSE are required. On Linux the runner prepares the repository's pinned securefs provider once. A distribution binary can be supplied directly.

```sh
python3 scripts/test-local-web.py --binary /absolute/path/to/cozy
```

For UI iteration, Vite serves current sources and proxies to the test Calcifer. Watchexec queues the next run when the source or fixture changes.

```sh
python3 scripts/test-local-web.py --vite --watch
```

Use `--securefs /absolute/path/to/securefs` for an explicitly prepared helper. `scripts/prepare-securefs.py` downloads and verifies the repository's pinned archive. `--harness` can select another checkout's vault fixture when comparing an unchanged binary with a fixture correction.

Each run writes its result, binary digest, timings and logs under `target/local-web/results`. Cleanup asks the owned Calcifer to close its views and stop. A process that cannot drain leaves its profile and diagnostics available for inspection.

This runner exercises the real Rust service and real Chromium vault flow. The cross-platform release jobs remain the qualification for installed Windows and macOS behavior. Other browser cases that execute functions also need an explicitly prepared local worker and matching Runtime environment.
