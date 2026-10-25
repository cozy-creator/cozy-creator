# Explicit GPU warm factory

This release-gate cell uses a real stored BF16 16×256 linear model, normal Model/Loader placement,
and a native prefix/repeat/suffix CUDA graph. It is mechanism qualification, not SDXL parity or a
performance benchmark. No inference handler can create its program; missing warm preparation is
an explicit failure unless the request asks only to inspect the held-back state.

Root runs this under the owned machine's normal stack/run guard with the candidate CLI and Runtime:

```sh
python3 release-gate/suites/execution/prepared-warm/run.py \
  --machine OWNED_MACHINE --model fidika/unique-prepared-warm-proof \
  --out /absolute/durable/receipts
```

The controller uses only default-home `cozy` commands, creates/uploads a tiny private checkpoint,
then explicitly warms one shape. Two changed inputs must reuse one process/definition with zero
new captures or factory calls, no Python phase callbacks, one native graph launch per request,
and actual output parity against the loaded model. A changed step count gets one new definition.
An impossible free-byte requirement checks an actual capacity refusal; three ordinary inspection
calls exercise the keeper without another factory attempt. A new explicit warm retries once.
Warm results must distinguish `prepared=false, level=host` from program readiness.

Additional required real cells, run by the root controller in its machine guard:

- Run explicit GPU warm with `hold_ms=30000` on a new step count. Wait for the actual
  `prepared-warm factory entered` log, then `cozy run cancel RUN_ID`. Record the exact executor
  PID from that log and prove it exits before a subsequent request uses the lane. A fresh warm
  and inference must use a new PID, with no delayed cancel able to kill it.
- Supply malformed `steps` or a forbidden factory injection in a separately installed package:
  the ordinary binder/static interface must refuse before GPU work. Keep those raw refusals.
- Weight-only `--warm gpu` with model choice and no scalar inputs must omit `prepared`. Imported,
  host and downloaded warming must retain their original scope; scalar input at lower levels
  is rejected.
- With a retained older Runtime lacking `program_preparation`, the explicit factory request is
  held back honestly without invoking the handler or silently mapping all weights. Weight-only
  warming remains usable. SDK selection/restoration belongs to the root, not this controller.
- Pressure retirement is a separate real gate: after the existing RAM/offload path retires the
  definition, ordinary inference must refuse/fallback safely and must not secretly recapture.

The CLI warms a whole desired set. Every explicit warm-set replacement is a new bounded attempt;
background keeper passes never retry an already attempted member. Readiness is a point-in-time
receipt, not a promise that a graph survives later pressure. No source/GPU pass is claimed here
until the actual receipts are saved.
