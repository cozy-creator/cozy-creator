# Required graph-capable offload: segmented adapter contract

Status: native ownership contract agreed; adapter implementation and actual CPU binding
checks are present. Scoped native readiness is an Agent 2 dependency, and a capture
helper opt-out from context synchronization awaits shared-file agreement. No GPU
execution, streamed residency or performance is qualified here. Product input is
`0af634eff39c1a28ea587e1b69516f9dc326d142`, packing upstream
`2c2d05c755195921f816719ab9e3aad39638d62d`, fixtures `3c678dfcd`.

## Frontend seam

```python
owner = vt.offload(model.eval(), device=0)
# Explicit off-request preparation for a fixed supported module boundary.
segment = owner.capture(
    model.blocks[0], (example_cuda_input,), kwargs={},
    stream=explicit_stream, reserve_bytes=prepared_pool_capacity,
)
with torch.inference_mode():
    result = segment(new_input, stream=actual_consumer_stream)
segment.close()
```

`segment.facts()` is passive definition metadata: `phase`, `region_handles`,
`version_dtypes`, `pool` and `closed`. It does not acquire residency, wait, or infer
streaming from a flag. The fixture checks actual native Pin/mapping state independently.

The first implementation captures an explicit module boundary whose
managed weights belong to this owner. It is the primitive for caching graphs at
inventoried boundaries, not a permanent eager-only exception. General pipeline
orchestration stays on the host; eligible tensor compute uses segments. Unsupported
dynamic control, mutation, shapes or operators report the unsupported boundary; no
failed user computation is rerun to hide an OOM. Automatic boundary graph selection
follows after this primitive is qualified, without Python residency/byte policy.

`reserve_bytes` is forwarded to existing `vt.graph` pool preparation; it is not an
adapter memory budget or eviction rule. Preparation is explicit and separate from
request execution. A shortfall is returned after proper capture cleanup, with no
automatic retry. Do not repeatedly resample a process-wide peak for every segment.

Inputs/kwargs have a fixed pytree, ordinary Tensor metadata and immutable scalar
configuration. The current signature requires nonoverlapping offset-zero, nongradient
inputs, rejects cross-input aliases and custom leaves, and permits ordinary Tensor
outputs only. Static inputs retain their ordinary Working storage; captured outputs
and temporaries belong to the graph pool. Both count under their existing owners,
without counting static inputs again as captured footprint. Inputs are copied into
their static storage before launch. Returned results are independent
working copies scheduled after the replay on its actual stream, so the next call
cannot overwrite a retained result. Gradient/training/stateful module capture and
unmanaged/custom pointer escapes need a separate explicit contract.

## Definition ownership

One `GraphSegment` retains:

- The CUDA graph definition/exec and existing pool/GraphFootprint owner.
- Exact immutable `_Version` identities and their Source recipes, including packed
  member offsets, shape, stride and storage-local offset.
- Deduplicated native Regions and a segment Plan; their stable VA reservations remain
  alive even when backing is absent. New dtype versions never relocate these Regions.
- Static working input/output tensors and signature/dispatch metadata.

The definition must **not** retain ordinary typed export tensors or native physical
Pins for its full cache lifetime. Existing `Use.tensor`/`Region.tensor` storage owns a
Pin, so retaining every prepared export would make the captured model fully resident.
Such a result is resident graph support only and must never be called streamed graphs.
Escaped user aliases independently keep their legitimate Pins and may prevent eviction.

Preparation uses existing public host/host-parts Source Regions, not Runtime checkpoint
I/O adapters or a second allocator. Packed Regions remain deduplicated; every typed
view adds its member offset only after original-storage span validation.

## Capture transaction

1. Freeze the segment's immutable versions and prepare its Region Plan outside capture.
   Establish exclusive adapter access; dtype conversion cannot mutate a live definition.
2. Prepare/warm supported compute and working pool outside capture. Acquire a public Use
   on the explicit capture stream and create all typed weight exports before capture.
   Native Source publication/orders precede reads on that actual stream.
3. Enter existing `vt.graph(graph, pool, stream, reserve_bytes, footprint)`.
   A private capture-binding context unwraps managed weights only to those prebound
   tensors. No Source work, typed export, native Plan activation, allocation recovery,
   event query/wait or new residency acquisition occurs inside capture. Nested declared
   forwards validate membership instead of switching the device-wide Plan.
4. End capture through existing routing/footprint cleanup. Only afterward release all
   temporary exports/Uses through normal consumer fencing. Reject hidden capture views
   or classify them explicitly as resident; never force-drop a Pin.
   Track the actual C10 Storage families across detached aliases. If any capture Storage
   still survives table release, retain that same Storage/export owner and the epoch,
   then refuse streamed classification. The scalar Pin census is diagnostic/test evidence,
   not an atomic ownership capability. Capture storage reassignment to an ordinary target
   and writes to prepared weight aliases are refused before mutation.
5. Publish the definition only after success. On failure keep exact captured/native
   owners through checked cleanup, retire partial metadata and expose the original
   error. Do not publish a half-initialized definition or execute an application retry.

Agent 2 confirmed this sequence, with the additional scoped readiness call:
`Use.wait_ready()` / `vr_v1_use_wait_ready(handle)` / Rust `&self wait_ready()`.
It waits current-generation Region tickets through checked completion and resource
retirement, including ProducerGuard release. It releases the GIL, preserves ownership
on refusal, and uses no global retirement drain/context synchronization. It does not
settle unrelated producers or promise capture admission. `export_use` calls
`gpu.eager`, so it must never be invoked during capture.
GraphFootprint accounts captured Working; it does not itself account weight residency.

The existing `vt.graph` currently context-synchronizes during preparation. The adapter
requires an agreed `synchronize=False` opt-out after scoped readiness and capture-stream
warmup completion; the default helper behavior remains unchanged. This helper amendment
has been requested but is not silently implemented in the adapter. Missing native/helper
capabilities refuse explicitly. `torch.py` is untouched pending agreement.

## Replay ownership

For each call, under the segment's submission lock:

1. Select the supplied actual stream, order input producers, and serialize access to
   static graph input/output storage behind the previous replay/copy completion event.
2. Activate the segment's exact native Region intent outside launch. Acquire Use for
   deduplicated Regions and fresh ordinary checked exports (one valid uint8 span per
   Region is enough). Existing TensorExport owns Pin/family/Weight and preserves exact
   final-fence quarantine. Bare Use close is not the checked replay safety contract.
   Initial replay also calls scoped wait_ready before launch to surface Source/resource
   failure; it does not claim fill/compute overlap or latency qualification.
3. Copy inputs, enter the existing `_runtime.replay` host guard, and call
   `vt.replay(graph, stream=stream)`. Reuse native pool retain/record/release ownership.
   Exit replay exclusion before output-copy allocation and before Use close/fencing;
   its eager ownership validation must not be invoked inside that guard.
4. Keep an extra existing native pool reference across launch and output-copy submission,
   even if replay hooks reset the executable. Enqueue independent output copies,
   record their pool/working tails, then release that reference. Record actual weight
   consumers and drop transient exports outside exclusion; their checked storage
   deleters, rather than bare Use completion, prove weight tails or quarantine.
   The original Use closes while checked exports still own independent Pins. Native
   last-consumer fences retain residency while work is pending after submission.
   A `record_stream` lifetime declaration never substitutes for a producer dependency.
5. Deactivate intent without publishing an invented compute key or successful H sample.
   If a known enclosing offload forward owns the device Plan, suspend it before selecting
   the segment and restore its exact phase afterward, outside exclusion. Never replace
   an unknown external Plan. Failed restoration retains parent/epoch ownership and the
   model's failed-intent state; nested capture/replay from an execution hook refuses.
   Failed/unproven consumer fencing retains native custody; it does not run a context
   join, force reference counts or retry the application.

The same cached definition can survive eviction/refill at the same VA between calls.
Only active replay Regions and legitimate aliases hold weight residency. Pools retain
their own graph/live-output/in-flight references under the existing mechanisms.
Closing disables new submissions and retires the definition/pool only under the
existing in-flight/consumer ownership contract; exact abandonment semantics are a
native confirmation item, not a Python finalizer assumption.

Old definitions retain old immutable dtype versions for already submitted work. Model
version/layout/configuration or buffer-address rebinding invalidates new selections;
prepare a new definition. Existing output copies and escaped old aliases stay valid.
The initial explicit primitive does not reuse derived Versions across dtype roundtrips
or automatically select/cache eligible model boundaries; those remain required model
integration work. Close unused/invalidated definitions to release their Working caches.

Private pools are currently required. Explicit shared pools refuse until ordered-group
replay/output semantics are implemented. Each cached Working pool retains its normal
backing while its definition/static owners live; only weight residency is replay-scoped.
No whole-model/scratch fit, 8 GB gate or universal stock-model graph coverage is claimed.
This is an implementable segment primitive, not a permanent eager-only exclusion.

## Minimal actual frontend fixture

The coordinator GPU fixture will use tiny real Torch layers, no downloaded weights:

1. Capture at least two packed module segments using the public adapter frontend and
   real `vt.graph`/GraphFootprint. Retain all definitions simultaneously.
2. Confirm stable Region IDs/VA and **zero graph-definition weight Pins after capture**.
   Holding a definition must not make the entire model resident. Keep an escaped alias
   as a positive control that its own Region remains protected.
3. Complete capture preparation, invoke existing native trim/pressure outside capture,
   and prove inactive unaliased backing can disappear while definitions survive.
4. Reacquire/refill and replay alternating segments on real explicit streams; compare
   outputs with eager Torch exactly or with a declared dtype tolerance. Preserve old
   output copies across later replays, and establish both stream order and lifetime.
5. Capture failure, Source failure, dtype-derived old-definition identity, reset during
   in-flight replay and final-alias retirement get their own negative/ownership controls.
   No short-circuiting the pressure, alias or memory assertions to manufacture a pass.

CPU work can validate signatures, metadata, immutable version selection and refusal
transactions on real Torch. It cannot qualify CUDA capture, stable-VA refill, pool
retirement or streamed memory savings. No GPU execution, rental, merge or release is
authorized in this lane.

The prepared actual frontend client is
`release-gate/suites/varena/torch-offload/graph_frontend_gpu.py`. It calls the proposed
public owner capture/replay API and the existing native scalar observer. It refuses
before CUDA initialization if the frontend is absent. It is unrun and is not proof of
CUDA readiness; missing native/helper capabilities still refuse. `graph_epochs_gpu.py`
adds actual C Source provider controls for source failure, unrelated producer capture
refusal, a delayed consumer gate and reset during launch. It requires explicit matched
provider Python/library paths and runs one isolated child per mode. No GPU mode ran here.

`graph_bindings_cpu.py` executes this same frozen dispatch path on real tied/strided
Torch and tiny Diffusers VAE inputs with exact eager parity and no new residency
materializations. It checks real weight/buffer/input mutation refusals, version/signature
invalidation and zero-element interior views. It simulates neither CUDA nor native
ownership; capture/eviction/replay/tail/reset proof remains coordinator-owned.

## Files and coordination

This lane owns new `_offload_graph.py`, graph docs and frontend clients in its durable
worktree. Planned shared `_offload.py` edits are the narrow capture-binding context,
dispatch/intent delegation and lazy owner capture method; `torch.py` graph helpers are
reused. Agree shared-file ownership with the coordinator/packing owner before edits.
Agent 2 owns any required native seam and signatures. Frozen #2449 is preserved.
