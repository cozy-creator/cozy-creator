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

The native wait retains an ABI retirement lease that excludes new capture/replay
admission on this device for the duration of the call. The wait set is scoped; the
operation is not zero-cost or concurrency-neutral. Completed/retired warm controls
take the native fast path. No adapter performance claim follows from that contract.

`vt.graph` prepares its pool outside capture, joins the caller/preparation stream onto
the explicit capture stream, then synchronizes only that capture stream. This is the
hard-cut helper contract; there is no whole-device barrier or synchronization opt-out.
Other producer streams must be explicitly joined by the caller before entry. Omitting
`stream` still creates a capture stream, with the same dependency on the actual caller.
The active Runtime and direct release-gate caller audit is in
`release-gate/suites/varena/torch-offload/GRAPH-CALLERS.md`. Existing capture/routing,
footprint, cancellation and exception cleanup are preserved. The thin public
`replay_scope` separately wraps only the existing native host replay guard.

Existing prepare_pool can still allocate/recover, trim or wait on reclaimable backing
under pressure; preparation is not globally nonblocking. Caller readiness must cover
static input/buffer contents, prebound weights, RNG/library/workspace initialization
and all capture-stream dependencies before completion is established. Use.wait_ready
covers only the selected native Sources. Atomic native admission can still refuse
unrelated active producers; the helper does not drain their streams or jobs.
The unrelated-producer regression prepares/warms the graph, exact explicit pool/reserve,
buffers and kernels before arming the gate, then passes the same pool/reserve so the
second preparation exercises the existing have>=wanted metadata path. It tests atomic
admission refusal plus caller ordering/compatibility/failure cleanup, not arbitrary-pressure
preparation latency.

## Replay ownership

For each call, under the segment's submission lock:

1. Select the supplied actual stream, order input producers, and serialize access to
   static graph input/output storage behind the previous replay/copy completion event.
2. Activate the segment's exact native Region intent outside launch. Acquire Use for
   deduplicated Regions and fresh ordinary checked exports (one valid uint8 span per
   Region is enough). Existing TensorExport owns Pin/family/Weight and preserves exact
   final-fence quarantine. Bare Use close is not the checked replay safety contract.
   Call wait_ready after the transient typed exports exist for every acquired capture
   and replay epoch, outside both exclusion guards. Producer ordering alone has no
   scoped acknowledgement for errors observed later by the producer/retirement worker.
   This initial wait surfaces selected producer/job/staging failure before graph work;
   consumer fences protect lifetime and do not substitute for that Source-success result.
   Future graph/device failures and final persistent Source/service teardown remain
   separate error/ownership paths. No overlap or latency improvement is qualified here.
3. Copy inputs, enter the agreed public `vt.replay_scope(device)` host guard, and call
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

This lane owns `_offload_graph.py`, graph docs/clients, the narrow `_offload.py`
delegation and the thin `torch.py` helper changes under root's single-writer grant.
These interfaces are implemented; no helper approval is pending. Ship the hard-cut
helper and graph frontend together in the same exact SDK cohort. Do not overlay the
frontend onto an older device-synchronizing helper or introduce a compatibility flag.
The adapter joins its original caller before switching to the capture stream, so that
edge remains present when the helper subsequently observes caller == capture stream.

Minimal native readiness adoption is implementation commit
`549f4915534c6f6d6840465b905d69038b07bbe7` only, the single-commit range
`b6caa5a053ea9a837877b7d327d78c71e56b1fa7..549f4915534c6f6d6840465b905d69038b07bbe7`.
`12425d44ff3de33b791f79937a2645c827529b51` adds optional real clients/contracts,
not another implementation dependency. Canonical input
`1eb4994755824648ac704a07d8efbc47d55de0f4` has identical Git blobs to b6caa5a05 for
all seven implementation paths, independently checked in this lane. No snapshot or
context/preservation prefix is required. The identity receipt is
`evidence/2026-11-02-readiness-adoption/native-readiness-minimal-adoption.json`.

Audited proof `b40f91f736a8a713229465c631e3b475428c6fb5` names its audited14e8/fetched7626
source. Its build, safety and host-client results are separate from canonical1eb499475
results, and neither is GPU proof. The earlier #2486 implementation/client mapping
186165d9f/1761c0612 and its receipts remain historical evidence for their named cohorts;
do not adopt context4463a4fd9. No further native ownership class, StageSource or unchecked
Tensor export is required. Frozen #2449 is preserved. Valid-Use GPU/graph proof is pending.

`vt.replay_scope(device)` is exactly the agreed thin wrapper: construct existing
`_ext._VarenaAllocatorCapture(device, replay=True)`, yield, close in finally. It adds
no driver call, changes to `vt.replay` or native exclusion semantics. It is host launch
exclusion, not GPU completion or weight/pool ownership. The separately granted capture
helper performs caller -> capture preparation and capture-stream synchronization only,
with unchanged admission/routing/footprint/error cleanup. There is no device-wide
fallback or synchronization opt-out. Native pool preparation may still wait/recover.
