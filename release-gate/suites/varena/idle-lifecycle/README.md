# Categorical idle executor retirement

This is an ordinary two-application GPU sequence, plus an optional unchanged real
model call. It installs nothing and never rents a machine. Root supplies an already
owned machine, the exact unchanged a96/7ad SDK facts and the GPU UUID. The worker
control is `53b015ccadc9580c6a29ad73760a4bce2c130fd1`; only the candidate adds cold
Cooperative Start retirement. Sampler27e and Runtime907 are separate, frozen work.

Each app retains a real16MiB GPU tensor across calls and reports PID/boot/start-tick
identity, parent identity, native owner incarnation, actual driver free/total and
storage pointer. The sequence runs A, A again, B, B again, A again. Repeated calls of
the same app must reuse its executor and storage. In the candidate, switching apps
must end the exact prior GPU executor while preserving its parent. In the control,
the prior executor must remain alive. The arm is explicit; a control pass records
retention and does not claim the lifecycle fix. Parent preservation qualifies an
import-only parent only when the full run receipt also reports fork launch.

```bash
python3 stage.py --out /absolute/bound-bundle \
  --runtime-version EXACT_RUNTIME --tensorfs-version EXACT_TENSORFS \
  --extension-sha256 EXACT_EXTENSION_SHA --gpu-uuid GPU-OWNED-UUID
python3 sequence.py --bundle /absolute/bound-bundle --out /absolute/control-results \
  --on OWNED --arm control \
  --model-target /absolute/unchanged/model/function --model-input /absolute/model-input.json \
  --model-arg 'model.base_model=EXACT_REF' --model-arg 'model.turbo_lora=EXACT_REF'
# After root installs the candidate worker, with the same SDK/source/input:
python3 sequence.py --bundle /absolute/bound-bundle --out /absolute/candidate-results \
  --on OWNED --arm candidate \
  --model-target /absolute/unchanged/model/function --model-input /absolute/model-input.json \
  --model-arg 'model.base_model=EXACT_REF' --model-arg 'model.turbo_lora=EXACT_REF'
```

Every call goes through default-home `cozy run --gpus=1`; complete NDJSON and
model arguments are passed verbatim as separate argv entries, with no shell parsing.
Use repeated `--model-arg` values to retain exact model manifests and other pinned
model-call arguments; for a value beginning with `--`, use `--model-arg=--FLAG=VALUE`.
The full command is saved in each progress receipt. Complete NDJSON and
`cozy run show` receipts are saved. A model failure remains a failed cell; it is
never replayed. Omit model arguments only for the short mechanism proof, and label
that narrower scope. H3 model/performance evidence uses the existing full15-second,
362-frame,1344×768,30-step/audio request, not a smaller surrogate.

Root must bracket the sequence with complete owned-worker memory logs. Candidate
`cooperative_idle_exit` notes must name the prior exact births, `exited=true`, no
termination/observation error and the matching new executor's `starting` birth.
The source orders these records before that executor's Start. The handler's later
birth observation alone proves exit by handler entry, not by Start.

Warm and active custody require separate real cells. Warm a declared model using
the ordinary `cozy run MODEL/FUNCTION --warm host --input ...` route, verify the
warm declaration succeeded, then start a cold probe and verify that model's exact
executor remains. Model-less probe apps are not substitutes for this warm-set
proof. Remove the task's own warm declaration with `--warm off` in cleanup. For an
active call, queue another app while the first still owns its lane and require the
first to complete unchanged; only its later idle incarnation may be retired.

The source preserves all explicit warm members and active custody. Real warm/active
and exact-before-Start receipts remain required before claiming those gates. No GPU
execution is claimed by fixture staging or AST checks.
