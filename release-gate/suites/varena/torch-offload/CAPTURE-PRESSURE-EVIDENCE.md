# Capture 26862 return evidence

Fixture-only addition to the existing instrumentation-002 diagnostic. No allocator or numerical-client change, reserve substitution, multiplier or inference retry. The original cap, two pipeline calls, seed/settings and assertions remain unchanged.

The Python return-event profiler observes the existing functions without replacing them:

- Every actual `vt.prepare_pool` return during the selected dispatches records pool, associated boundary device, original device argument, requested bytes and the actual returned integer capacity. A return event carrying `None` during exception unwind is explicitly unavailable, never zero. Pre-body preparation and the graph helper's later preparation remain separate real calls; the observer adds no calls.
- The complete original `_Observation.close` return retains per-observation peaks, `successful_backing_peak_bytes`, publication/error metadata and all other native fields. The entire stored `vt.phase_stats(device, native_key)` dictionary is also queried for every observed key outside capture. Stored `backing_peak_bytes` is not renamed or substituted for a reserve.
- Rows buffer outside actual capture and serialize at the existing dispatch/process-end points. Native exception snapshots continue buffering in the original C++ helper. Same-lock stream-usable total/largest/live/blocked evidence is explicitly missing. Post-return snapshots remain post-lock observations.

The profile slot must be unused before installation and is removed in `finally`. Collection errors mark diagnostic evidence; they do not replace execution. The profile adds host overhead and may perturb free/poll timing. That timing is a possible mechanism, not an established cause of 26862.

## Prepare a new diagnostic package copy

Keep all original directories and receipts. Use the coherent instrumentation-002 parent bundle SHA256 `ef52cd1dcf7132bd1df2e908b7108b9dab44eff4d4797f371d34fba959e0134e`; this patch deliberately requires its exact source. The original instrumentation-001 remains unchanged.

```bash
TASK=/home/fidika/cozy/.worktrees/cozy-creator/xs138-p4-auto-graphs/release-gate/suites/varena/torch-offload
PARENT=/home/fidika/cozy_v2/outputs/xs138-capture-26862-review-20261103/instrumentation-002/diagnostic-bundle.tar.gz
DIAGNOSTIC_COPY=/absolute/new/operator-owned-directory
mkdir "$DIAGNOSTIC_COPY"
tar -xzf "$PARENT" -C "$DIAGNOSTIC_COPY"
patch --batch --fuzz=0 -p1 -d "$DIAGNOSTIC_COPY" < "$TASK/capture_pressure_evidence.patch"
cp "$TASK/capture_pressure_evidence.py" "$DIAGNOSTIC_COPY/"
uv run --no-project python "$DIAGNOSTIC_COPY/seal_trace.py" \
  --tensorfs /absolute/actual-installed/tensorfs \
  --source-sha EXACT_SDK_SOURCE
```

Operator binds SDK257/Runtime792 or its qualified successor, the actual extension basename and every new source/manifest hash into the ordinary Cozy package/child provenance. Copy this diagnostic's `trace_client.py` to that new package's existing `automatic_sdxl_gpu.py` slot, with its helper, numerical client and manifest alongside it; use the existing `stock_sdxl_auto` runner and original input. Do not run the numerical script directly or rent from this lane. No C++ rebuild or product overlay is required.

The numerical `original_sdxl.py` must stay SHA256 `e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652`: preserved managed client plus only ab5 native-sample telemetry. Sealing checks that identity. The actual 6 GiB failure remains red until a separately qualified product correction exists.

## Read the evidence

Each `up1-trace-*-dispatch-end.json` and process-end trace includes `return_evidence`: actual preparation returns, unfiltered phase-close observations, full stored samples, collection errors and explicit missing fit eligibility. Existing trace format 2 and exception-ordinal association are unchanged. Reject incomplete diagnostic evidence; do not interpret unavailable values as zero.

372196352 bytes is 354.9541015625 MiB. Its difference from 220200960 is 144.9541015625 MiB. Actual 26857 automatic call 2 reuses call 1's single cold sample; it is not another independent peak observation. Eager receipts do not contain native cold samples. Opaque native keys include process-local identities, so matched cross-process controls are not byte-equal native keys.

CPU client: `capture_pressure_evidence_host.py --output FILE` uses actual Torch/SDK functions to observe pre-install refusal without initializing CUDA; it creates no fake resources. Successful capacity returns, full successful phase-close observations, model loading and capture require the coordinator's GPU gate and remain unrun locally. Source checks preserve the parent helper binary and numerical client byte-for-byte. These checks do not establish a fix or a timing win.

If demand/geometry traces remain ambiguous, p3/root agreement is required before any same-lock core diagnostic. The proposed minimal record belongs after `take`/`fit_pool` refuse under the existing Working lock: original/rounded request, pool/stream identity, exact backing, usable free total/largest and live/blocked ranges with actual predicate reasons. No driver query, polling, state mutation or retry is part of that report.
