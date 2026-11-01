# Replay observations — implementation contract

This follows the eager measurement foundation in Creator #2353. Production prefetch and split
policy remain unchanged until the committed execution signal is qualified.

`vt.capture_footprint(device, pool)` creates an inert recorder. `vt.graph(..., footprint=...)`
starts it after preparation/synchronization and before `capture_begin`, then seals it only after
successful `capture_end`. Failed recording invalidates it. The trace records allocator-owned
allocation ranges, including scratch freed during capture; it does not read a process peak or
use the pool reservation as demand. Shared-pool traces union ranges/pages. Existing live
inputs/output aliases stay charged globally and the recorder adds no physical pin.

A PreparedRepeat retains the sealed token tuple for its actual prefix/body/suffix graphs.
`PhaseController.wrap(..., footprints=tuple, execution_key=None)` keeps the new semantic execution
key separate from the existing weight-preference identity. Native start/observe/rearm owns the
stage observation. Only checked successful completion publishes the capture trace as validated
replay demand. Cancel, failure, unknown keys and unsealed/missing traces preserve the prior
successful sample. Actual allocator work outside the trace is measured independently.

Pool incarnation and device are validated before use. Recording-token metadata lifetime does
not prolong graph backing. The graph/pool/output/stream owners retain their existing lifetime
contract; no callback or Python work is introduced in the native stage replay loop.

`controller.facts()["observations"]` reports terminal stage rows once for Runtime telemetry.
Runtime retires semantic keys with their model owner, and removes its old outer non-publishing
replay observation only when the native wrapper owns every stage. Graph-footprint and real
continuation CUDA qualification remain pending until recorded explicitly.

Replay records are keyed by the semantic configuration plus the ordered monotonic identities
of the exact graph definitions. A fresh capture does not reuse the old record merely because
shapes match: opaque library dispatch may differ. Graph reset/finalization removes records that
contain its identity even if another graph retains the shared pool. `phase_forget(semantic_key)`
also removes all definition-scoped records for that model/plan key.

The callback interval is explicit: `with vt.external_callback(device):` performs no CUDA work.
Actual working allocations/frees during that interval invalidate the active observation before
mutation, including a new native stage after rearm. Enter/exit alone does not invalidate CPU-only
callbacks. Its counter is not loaded on the inactive-measurement allocator path. The interval
conservatively includes concurrent worker allocations; detached work after callback return is
outside the callback's owned scope. No second deferred-publication lifecycle is introduced.

The raw admission API is still eager-only in this checkpoint. Replay records and known stage
ownership supply the next cutover's evidence; they do not silently enable production speculation.

Omitted `footprints=None` preserves the original preference-only wrapper; explicit tuples,
even empty tuples, opt into native observation. This avoids nesting a new observation inside
an older Runtime's outer non-publishing replay scope. Missing traces still cannot publish.
