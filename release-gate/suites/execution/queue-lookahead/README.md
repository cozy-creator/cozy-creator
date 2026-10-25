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
Cancellation modes wait for B's explicit `sent` event with its public `run=` ID, then refresh A
through ordinary `run show` and check its GPU-return/terminal events before canceling. There is
no fixed delay used as evidence that preparation started. If A's active window ends first, the
controller saves `unexercised.json` and fails that case. Final machine timestamps must still show
queued-hint cancellation before A completed.

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

Installed RTX3060 cohort Runtime`de3dfc42c`/TensorFS`6431a5387`/TensorD`9998cf5f4` completed
all seven scenarios' jobs. The original controller incorrectly compared public run IDs with
numeric executor request IDs and reported failures; those red logs remain preserved.
Authoritative `cozy run show` memory request IDs permit posthoc qualification of **handoff
17567–17569**, **overlap17570–17572**, and **incompatible17585–17587**, including actual
sent/ready/reused timing and ordered correct results. [Exact partial proof](evidence/partial-20261025.json).

Cancel, abandon, refusal and active-cancel scenarios retain terminal/PID isolation evidence,
but their pre-entry canceled/refused or killed requests lack mapping in the old returned
measurements. Those old receipts remain **partial**, not passed. No mapping is inferred from timestamps or
adjacent numeric IDs. The future fixture returns Context.request_id for entered work and reads
normal run-show measurements; pre-entry cases require an explicit public-to-executor ID mapping.

The corrected fixture `b37bbc671` subsequently passed all four negative modes on RTX3060,
Runtime`07d5e7054`/TensorFS`16c857cbf`/TensorD`fd4ff48fa`:

- Cancel **17625–17627** and abandon **17628–17630**: explicit B sent/canceled mapping;
  cancellation before A completed; B never entered; A completed and C used a fresh PID.
- Refusal **17632–17634**: B's exact failed preparation was reused, without handler entry;
  A and C completed in the same executor.
- Active cancellation **17636/17637/17639**: B's mapped hint was observed while A was active;
  A was canceled; B and C completed correctly in another executor.

[Exact negative-mode proof](evidence/negative-20261025.json) records artifact hashes, all public
and executor IDs, before-cancel observations, worker timestamps and raw receipt hashes. These
are fresh runs; the earlier three posthoc passes and four partial cases are preserved separately.
No external-pressure, multiple-GPU or GPU-weight-prefetch qualification follows from these cells.
