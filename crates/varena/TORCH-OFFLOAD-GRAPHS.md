# Required graph-capable offload: segmented adapter contract

Status: proposed implementation contract; native ownership confirmation pending with
Agent 2. Graph support is required remaining work. Nothing here declares GPU execution,
streamed graph residency or a performance win. Product input is
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

The first implementation captures an explicit module or pure callable boundary whose
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
configuration. Working static inputs and outputs belong to the graph pool. Inputs are
copied into their static storage before launch. Returned results are independent
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
5. Publish the definition only after success. On failure keep exact captured/native
   owners through checked cleanup, retire partial metadata and expose the original
   error. Do not publish a half-initialized definition or execute an application retry.

The prebind/drop/reacquire sequence needs Agent 2 confirmation for the current public
ABI: `export_use` currently calls `gpu.eager`, so it must never be invoked during capture.
GraphFootprint accounts captured Working; it does not itself account weight residency.

## Replay ownership

For each call, under the segment's submission lock:

1. Select the supplied actual stream, order input producers, and serialize access to
   static graph input/output storage behind the previous replay/copy completion event.
2. Activate the segment's exact native Region intent outside launch. Acquire Use for
   its deduplicated Regions; native Source preparation orders producers on this stream.
3. Copy inputs, enter existing replay exclusion if required by Agent 2, and call
   `vt.replay(graph, stream=stream)`. Reuse native pool retain/record/release ownership.
4. Enqueue independent output copies, record the actual consumers, and close Use only
   after submission. Native last-consumer fences retain residency while work is pending.
   A `record_stream` lifetime declaration never substitutes for a producer dependency.
5. Deactivate intent without publishing an invented compute key or successful H sample.
   Failed/unproven consumer fencing retains native custody; it does not run a context
   join, force reference counts or retry the application.

The same cached definition can survive eviction/refill at the same VA between calls.
Only active replay Regions and legitimate aliases hold weight residency. Pools retain
their own graph/live-output/in-flight references under the existing mechanisms.
Closing disables new submissions and retires the definition/pool only under the
existing in-flight/consumer ownership contract; exact abandonment semantics are a
native confirmation item, not a Python finalizer assumption.

Old definitions remain bound to old immutable dtype versions. A module conversion can
produce a separate definition; it must not rewrite an existing executable's pointers
or silently select new weights. Model close/definition close ownership must be explicit.

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
an implemented API; native/shared-file confirmation precedes dependent implementation.

## Files and coordination

This lane owns new `_offload_graph.py`, graph docs and frontend clients in its durable
worktree. Planned shared `_offload.py` edits are the narrow capture-binding context,
dispatch/intent delegation and lazy owner capture method; `torch.py` graph helpers are
reused. Agree shared-file ownership with the coordinator/packing owner before edits.
Agent 2 owns any required native seam and signatures. Frozen #2449 is preserved.
