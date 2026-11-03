# 27056 host-stage diagnostic

Diagnostic only. The original 27056 GPU red remains red. No kernel fault, GPU completion, or fix is established by a normal host return.

This feature adds files only under this directory. It does not import the 9d/627 product context into its Git branch. The selected runtime is checked against exact source and native file hashes. The callback attachment is a process-local fixture proxy: the original callback body is called once and its result/exception is preserved. No shared native marker ABI exists.

## Prepare without build or install

Use an existing qualified SDK package directory. Choose one complete product context; do not mix Python helpers and native images.

```sh
python3 -I -B prepare.py \
  --cohort 9d \
  --sdk-root /existing/qualified/wheel-tree/tensorfs \
  --numerical-client /existing/e7c/original_sdxl.py \
  --output /new/owned/host-stage-ready
```

For root's separately qualified guard composition, use `--cohort 627` with its matching SDK directory. The binder verifies all six Python files, copies the unchanged e7c body and diagnostic files, and records the three actual native-image hashes. No SDK import, installation or native build occurs.

## Operator-only actual GPU command (UNRUN here)

Inside the already isolated, matching Torch2.15+cu132 SDK child, use the **same original e7c arguments**:

```sh
python -I -B /new/owned/host-stage-ready/entry.py \
  --model "$FROZEN_SDXL_MODEL" \
  --graphs auto --device 0 --steps 20 \
  --height 1024 --width 1024 --seed "$ORIGINAL_SEED" \
  --guidance "$ORIGINAL_GUIDANCE" --prompt "$ORIGINAL_PROMPT" \
  --output /new/owned/results
```

Supply the accepted request's exact original seed, guidance, prompt and model path. The e7c body performs two full calls itself; it has no --calls option. The original body itself enforces the 6GiB managed cap/preflight and original assertions. No numerical body, cap, pool target or application retry is replaced. The entry point requires fresh CUDA initialization state just as the previous diagnostic did. It records the current environment flag only, not a guessed latched Torch setting.

The data window includes the following component cleanup; normal progress closes it at up_blocks.2 dispatch3 entry, while errors keep it open through final unwind. Lifecycle checks continue afterward. The ring writes `host-stage-ring.json` only after the observed capture/replay scopes have confirmed normal exit. Unknown exclusion, profile ownership loss, callback contention, overflow or sink failure produces the pre-serialized `VARENA_HOST_STAGE_UNAVAILABLE` stderr marker, without replacing the original application failure. A returned Use/replay/tail-install record means host return, never GPU completion.

The new launcher deliberately owns one profile hook and replaces the old Python ReturnEvidence/dispatch wrapper. Do not stack it with that wrapper. Root/native owner's C++ allocator tracer is a separate composition task; this feature does not install, modify or duplicate that tracer. No optional synchronization control is present.

```sh
python3 -I -B analyze.py \
  --ring /new/owned/results/host-stage-ring.json \
  --output /new/owned/analysis.json
```

The analyzer preserves raw zero IDs and separately derives native cache-owner IDs from scalar mappings observed on existing product-facts returns. It does not call facts itself. An earlier legitimate failure can leave the requested late interval incomplete; that is reported rather than manufactured as complete.

## Host/source checks

```sh
python3 -I -B host_check.py --output /new/owned/host-controls
python -I -B host_check.py --output /new/owned/cpu-tensor-controls --torch-cpu
python -I -B sdk_cpu_check.py --cohort 9d --output /new/owned/sdk-cpu-controls
```

The last command needs the explicitly sealed SDK and Torch d0f source cohort. It executes a real three-block CPU Offload application three times, checks the actual late host window, outputs and collection while the ring remains alive. It does not create a native GPU CacheOwner or capture/replay.

The generic host controls use real files, the actual CPython `_testcapi` foreign-thread helper, real Python profile ownership and real CPU Linear/C10 Storage lifetime. A failed early control incorrectly assumed CPython3.12 cProfile occupied the legacy sysprofile slot; its receipt is preserved. The corrected control uses the actual legacy `profile.Profile` slot. None of these controls qualify native GPU stream/pin/allocator safety.

The saved original trace demonstrates the missing interval with an expected nonzero verdict:

```sh
python3 -I -B analyze.py --saved-27056 /existing/27056/execution-001 \
  --output /new/owned/original-gap.json
```

`MISSING_LATE_INTERVAL` is a host analysis of saved evidence, not a new GPU reproduction. All actual GPU runs remain root/operator-owned and unrun by this lane.
