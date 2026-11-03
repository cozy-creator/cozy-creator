# 27056 shared host/native marker diagnostic

Current draft uses interpreter monitoring, without product callback/function wrapping. The original27056 result remains red. Normal return is host return, not GPU completion. Existing `evidence/host-source.json` is an archived v1 reference; current monitoring evidence is separate.

Development context5a196232c is fresh master500b plus627, not an execution/adoption claim. Adopt only the new diagnostic files/feature commits after that context. Execution must be explicitly sealed to original9d or root's controlled-b91 artifact. No code executes implicitly from development master.

## Prepare a new artifact copy (no import, install or build)

```sh
python3 -I -B prepare.py \
  --cohort 9d --artifact-label 9d \
  --sdk-root "$EXACT_9D_SDK_PACKAGE_DIRECTORY" \
  --numerical-client "$UNCHANGED_E7C_CLIENT" \
  --helper "$NATIVE_OWNER_HELPER" \
  --helper-sha256 48b74e9080e171713126888a0fcd62e8815351e70c1ae4d8db7788f91a2a8955 \
  --header "$NATIVE_OWNER_FINAL_HEADER" \
  --output /new/owned/shared-host-stage-ready
```

Header SHA must be2551e703227f6bda7f9ecc10231217bac552097712d2fee6840becf2a16940ad. The native helper/header are copied byte-for-byte, never edited or built here. For the separately selected guard artifact, use `--cohort 627 --artifact-label controlled-b91` and its exact SDK package; do not reuse9d native image hashes. The binder checks all six Python source files, records the actual three native images, and retains the exact e7c body SHA. It cannot qualify an SDK build merely from a label.

## Operator-only execution (UNRUN by this lane)

Use the accepted request's exact original arguments:

```sh
python -I -B /new/owned/shared-host-stage-ready/entry.py \
  --model "$FROZEN_SDXL_MODEL" --output /new/owned/results \
  --device 0 --graphs auto --steps 20 --height 1024 --width 1024 \
  --seed "$ORIGINAL_SEED" --guidance "$ORIGINAL_GUIDANCE" --prompt "$ORIGINAL_PROMPT"
```

The e7c body itself performs two complete calls and the unchanged6GiB cap/probe/assertions. It has no --calls argument. It is invoked once. The native getter records Torch's actual latched workspace flag before CUDA; cache-enabled or launch-blocking configurations refuse this asynchronous diagnostic before body execution. No flag is mutated and no synchronization/checkpoint control is added.

This entry replaces the old Python trace/ReturnEvidence wrapper. Do not stack another helper/allocator wrapper or profile onto it. It observes the original `_bind_allocator` normal return to install the supplied single maintained native tracer before CUDA. Product functions and callbacks remain original code. A failed helper/marker is explicit incomplete evidence, never an authored retry.

## Marker and lifetime rules

`allocation_trace_marker` has exactly the agreed eight scalar arguments. Root's IDs100..203 are used unchanged; snapshot markers1..8 remain separate. Each host row stores the returned common native ordinal; requested marker ordinal0 marks evidence unavailable/lost. The new host-v2 row has the previous20 scalar fields plus native marker ID and ordinal.

`segment_id`/shared `graph_identity` is a monotonically assigned diagnostic GraphSegment token using weak keys. It is not a CUDA handle. Public Plan handles are not native PlanIDs. Missing identities remain0/-1 with explicit reasons. Cache native IDs come only from existing safe product-facts returns/admission records; no byte-delta reconstruction occurs.

CPython3.12 monitoring reaches fresh C-created Python threads in actual host controls. Original callback transitions are observed without proxying `_shrink`. PY_RETURN, PY_UNWIND and PY_YIELD have distinct semantics; None is never used to infer success. Copy/clone C-call events are observed without replacing tensor operations. Actual native GPU callback markers still require returned execution evidence.

The window begins at up0 invocation2 and persists across the gap through invocation3 and following up1 cleanup/error. Normal progress ends at up2 invocation3 entry. Native window1 remains open across the gap while stage0 disables ATen there. Only selected up0 bodies enable ATen; primitive native map/retire records continue through the gap. All stage-return events are host observations.

Static/output/buffer VA and footprint metadata is collected once after confirmed capture exit. Existing selected-Region census returns are copied; no new census polling is added. The collector strongly retains no Tensor, Storage, frame, Owner, graph, Plan or Use. Weak identity keys do not keep graphs alive. Metadata snapshots and retire-intent events do not prove physical residency/release.

`host-stage-ring.json` and the helper's untouched format3 `native-host-stages.json` are written only after confirmed exclusion exit. Native dumping has no phase_stats dependency. Host loss can leave useful explicitly partial native evidence; unknown exclusion prevents both dumps and emits a pre-serialized unavailable marker. The native row_fields/kind schema is never replaced with legacy17-column metadata. Kind16/SEGMENT_FREE is PRE-unmap retirement intent, not completed physical release.

```sh
python3 -I -B analyze.py \
  --ring /new/owned/results/host-stage-ring.json \
  --native /new/owned/results/native-host-stages.json \
  --output /new/owned/analysis.json
```

The analyzer checks common ordinals and exact marker/stream/graph/ABI-Plan/pool values. The native owner joins admission request generations/nativePlanIDs; overlapping/ambiguous spans remain unbound. No event-query or normal return is declared the original fault site.

## Host/source checks and limits

```sh
python3 -I -B host_check.py --output /new/owned/host-controls
python -I -B host_check.py --output /new/owned/tensor-controls --torch-cpu
python -I -B sdk_cpu_check.py --cohort 9d --output /new/owned/sdk-controls
```

Controls use real IO/generator errors, actual CPython foreign C threads, real CPU copy/clone shape refusal, C10 storage/weak identity collection and existing-tool refusal. The exact9d CPU application checks the named window and following cleanup. Target CPython3.12 CUDA graph/native callback ownership is unrun; a CPU/source result does not qualify it. Native helper build/host proof is owned separately and is not repeated here.

Old v1 receipts and the failed monitoring setup control are preserved. No product policy, cap, source retry, forced references, SDK build/install, GPU/rental/machine action, merge or release is performed in this lane.
