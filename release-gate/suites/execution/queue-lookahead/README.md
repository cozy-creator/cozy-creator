# One queued request, actual executor handoff

This is a manual default-CLI correctness gate for TensorD request lookahead. It uses real
deterministic CPU preflight and a real native CUDA repeat; no model mock or private SDK
interpreter. It neither changes SDKs nor rents machines. The CPU preflight performs bounded
PBKDF2 computation without I/O or GPU work, making its in-flight cancellation reproducible.
Its returned facts are deterministic. The handler reports its actual process and timestamps.

After installing the paired candidate, install the fixture once before starting the queue
observation window. Dependency unpacking is setup, not a queue failure:

```sh
cozy run /absolute/worktree/release-gate/suites/execution/queue-lookahead/gpu \
  --warm installed --on MACHINE --json
```

Then run each mode with a new seed and output directory:

```sh
python3 release-gate/suites/execution/queue-lookahead/run.py \
  --on local --seed 179901 --mode handoff --out /absolute/durable/queue-handoff
```

- `handoff`: immediate B preparation, then A→B→C. All succeed in the same executor and
  enter in order. This exercises the gap between journal claim and worker-thread promotion.
- `overlap`: B does substantial genuine CPU preflight while A's handler is still open.
- `cancel`: cancel queued B during preflight; A completes, B never enters, C succeeds.
- `abandon`: B's CPU preflight is deliberately long; cancellation cannot indefinitely
  block C. A completes first, then the exact executor is retired and C uses a fresh one.
- `refuse`: B fails preflight without entering its handler; A and C succeed in the same
  executor. The refusal belongs to B.
- `cancel-active`: ordinary cancellation kills A's executor; B and C still succeed, and
  B uses another process. Stale preparation cannot survive the dead process.
- `incompatible`: B names another entrypoint/construction before compatible C. B must
  retain its queue position; C must not be prepared by skipping B.

The driver preserves every CLI event/receipt and cancels only its own unfinished jobs on
failure. It checks outcomes, exact numeric output and handler entry order. For canceled
hints it also requires retirement after the active call. It does not claim preparation
overlap from total duration or completion alone.
The initial setup/first-handler window is separate (at most15minutes); the180-second queue
observation deadline begins only after A enters. The enclosing rental guard remains authoritative.

The driver saves worker TensorD diagnostics through `cozy machine logs MACHINE tensord
--lines 0 --json` as `tensord.ndjson` and validates the request IDs and timestamps. Attach
these matching diagnostics as part of `## Proof`: `request lookahead
sent`, `ready`, `reused`, and `canceled`, with their request IDs. For `overlap`, B's `ready`
must precede A's completed timestamp; for `handoff`, B and C must actually reuse their
preparation when offered. Confirm no more than one pending hint and no C hint across the
incompatible B queue barrier. A canceled hint's executor may retire only after A finishes.
If the race window was missed (for example B was canceled before any hint was sent), the
cell is inconclusive and must be repeated with its traces, not reported as passing.

No GPU weights are preloaded by this feature. Preparation remains normalization/preflight;
the existing scheduler alone authorizes Invoke and owns device permits. Ordinary dispatch
remains unchanged for executors without the `request_lookahead` Hello capability.

Source checks only so far. No installed-cohort run has been recorded.
