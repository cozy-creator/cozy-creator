# Late-ready queued preload (Runtime #1838)

A manual default-CLI gate: request A runs a native GPU program (a resident repeat or an
all-resident two-stage sequence) while queued request B finishes its CPU preflight late. A's
production `Execution.poll` must offer B's one immutable raw block while A still runs; B then
reads it from the same Region with zero demand bytes. Each run uploads its own private 64 MiB
source commit, so every case needs a new seed and output directory:

```sh
python3 release-gate/suites/execution/queue-late-ready/preload-run.py --on MACHINE \
  --model ACCOUNT/PRIVATE-REPO --seed SEED --program repeat|sequence \
  --mode control|overlap|capacity|cancel --out NEW_DIR
```

`control` has no queued work. `capacity` holds real Torch ballast so the offer is a recorded
capacity skip and B demand-loads. `cancel` cancels B by the ordinary CLI while A runs; a
replacement C then loads on demand in a new executor. An already completed fill is reported
as such: active-I/O cancellation is not claimed.
