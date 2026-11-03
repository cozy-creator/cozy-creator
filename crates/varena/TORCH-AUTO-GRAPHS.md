# Automatic immutable offload graph segments: initial contract

Assigned implementation lane: P4; draft contract precedes shared adapter changes.
Base: fetched master970be78c7. Product prerequisite: the current eb315 graph/helper
with graph hash1462072, ordinary checked exports, scoped readiness and root-selected
native measurement/cache-owner implementation. No old native snapshots are adopted.
GPU qualification, byte policy, native interfaces and canonical integration remain
separately owned. This document is a concrete implementation proposal, not GPU proof.

## Entry points and boundary selection

`vt.offload(model_or_pipeline, device=0, *, source=None, graphs="auto")` enables
automatic segments by default. `graphs="eager"` is an explicit diagnostic opt-out;
CPU remains actual eager inference. No caller pool target, memory cap or reserve-size
estimate is accepted for automatic mode. Existing explicit owner.capture remains.

An owner-local controller delegates only from the existing declared-boundary forward
wrapper, before invoking its authored body. Select a nonoverlapping frontier of the
existing inventory boundaries; a selected boundary suppresses automatic selection in
its descendants. Structural declarations are not new weight/pool byte policy.
Do not capture the whole pipeline or combine all model weights into one execution.
Composite fallback roots without a useful segment cut stay eager with an explicit
eligibility reason in this slice; simple atomic declared modules can be segments.
No automatic direct-weight read interception or rewriting of model forward code.

`owner.graph_facts()` exposes passive states/reasons and native measurement IDs/peaks,
not Tensor/Region/Plan/executable handles that could escape cache lifetime control.

## Immutable key and eligibility

Per boundary, retain at most one current signature/definition. A key contains boundary
identity, exact immutable Version/OPEN Region/member identities and layouts, parameter
and buffer metadata, supported scalar/immutable-tuple module configuration (including
Conv fields), forward identity, input pytree/shape/stride/dtype/device/static scalars,
inference mode and relevant library/workspace configuration. No data pointers from
transient inputs become key ownership. Static Working owners belong to the definition.
Freeze only ordinary tuple/list/dict input structure and immutable scalar mapping keys;
do not retain TreeSpec custom contexts or Tensor mapping keys as hidden input owners.
Use non-reused owner generations in opaque native measurement keys, not a recycled
Python owner address. Version/Region handles and pack offsets remain exact descriptors.

Inputs must satisfy the existing GraphSegment ordinary/nonoverlap/immutable-scalar
contract. BEFORE any capture attempt, require reviewed module effects and the actual
recorded original/installed forward identities. The initial reviewed set is deliberately
small: standard unmodified Torch Linear/Conv/normalization/activation/container forwards.
Reject inplace=True, training, unknown custom/side-effect/buffer-mutating module classes,
new/replaced forwards, custom module/global hooks and unadapted Tensor/call access before
a warmup/capture body could execute again. Adoption must record actual forward origins;
do not infer them by trusting __wrapped__ annotations after a user override. Boundary
declarations, successful native measurement and independent outputs do NOT prove purity.
Third-party/custom forwards/control flow/global monkeypatches need separate reviewed
adapters; no general Python side-effect or kernel eligibility claim follows from a pass.

After the original cold eager call executes ONCE, inspect its actual output Tensor
identity/C10 Storage alias metadata, without cloning, reading values, making weight
reservations or retaining result objects. Auto is INELIGIBLE when any output aliases
an input, managed parameter/version, buffer, or another output leaf (including the
same Tensor returned twice and disjoint views sharing Storage). Scalar/custom output
types or unsupported/repeated container structure also require an adapter. Empty
Tensor data pointers are not alias proof; identity/Storage distinguish independent
zero-sized tensors from repeated empty aliases. No alias reconstruction is implemented.
Return that original cold result unchanged and retain the ineligible reason for the
same immutable key; later invocations remain ordinary eager, never independent clones.

This is separate from explicit GraphSegment, whose documented independent output-leaf
copies remain unchanged. Promotion requires BOTH reviewed effect eligibility and safe
cold output classification. An alias-free cold result cannot authorize an unknown
mutating module, and the read-only capture guard is not a substitute for this preflight.

Known enclosing intent of this SAME offload owner can be suspended/restored using the
existing epoch-owned Plan mechanism, outside exclusion. Extend only the private auto
preparation path to admit that known parent, and refuse a dirty/unknown parent before
warmup. Never clear the parent's frame stack to hide it or steal an external Plan.
An unexpected native active-Plan/phase refusal is surfaced or classified BEFORE body;
it is never caught after execution to silently retry/fallback.

## First-call measurement, then capture

The separate eager component-stream helper is Agent2-owned; this lane does not
duplicate or edit its new file. Compose its eventual exact API outside automatic
dispatch: component input/output scope -> measured eager/graph dispatcher on the
actual selected stream -> one outer output handoff. Reuse that selected stream and
the helper's checked join/completion information. If actual caller-stream changes
leave no scoped completion proof, return the once-executed cold eager result but
invalidate/abort its native observation; do not promote a guessed sample. Lifetime
recording alone is not an ordering/completion witness. Never redirect an explicit
stream change back to an assumed owner stream. Capture bindings bypass eager helper
joins/records; their frozen exports/stream ownership remain authoritative. VAE
decode/encode entry coverage is separate from a root.forward-only wrapper. Exact
minimal shared wiring will be composed by this lane after Agent2 provides its helper
interface; no second stream helper or speculative per-op fence removal is introduced.

The state machine is cold -> measured -> prepared/cached; a changed key retires the old
selection. No invocation executes eager as a retry after capture/replay has begun.

1. Preflight the key/eligibility and known native intent before any authored body.
2. A cold eligible signature executes its normal eager body exactly once under native
   `vt.phase_enter(device, key, mode="eager")`. After checked explicit-stream completion,
   call `vt.phase_leave(..., completed=True)` and retain only a published matching
   native charged-capacity sample. Abort/invalidate on any error; never train a failed
   observation. Outputs remain the real eager outputs of this invocation.
3. On a LATER invocation with the same immutable key, pass that native sample's
   `successful_peak_bytes`/`phase_stats.peak_bytes` verbatim as the pool reserve target.
   No Python multiplier, rounding, estimated scratch, global allocated_peak or predicted
   weight budget. Existing native prepare_pool rounds/adopts/refuses capacity.
   The eager sample is an observation, not a fit promise or a current closed allocation
   declaration; do not mislabel it as commit_execution capability.
4. Prepare static inputs and selected weights, Use+typed exports+wait_ready outside
   capture; use existing GraphSegment read-only warmup/capture and error cleanup. If
   preparation/capture executes and fails, propagate the original failure; do not
   re-execute the request eager or adjust the target and try again. A native pool
   shortfall remains visible instead of becoming Python OOM policy.
5. A published cached entry replays using fresh selected weight exports, per-Use scoped
   readiness, host replay guard, extra pool ref through output-copy tails, independent
   outputs and checked release. Definition owns descriptors/VA/Plan/static Working,
   never weight exports or permanent physical Pins.

Native phase observations are device-wide/nonoverlapping. split_context does not prove
that no eager parent observation exists. Measurement uses phase_enter(mode="eager");
automatic graph preparation/replay instead holds its OWN existing native preparation
observation (key=None, mode="preparation") until host preparation/launch/copy-tail
submission ends, then phase_leave(completed=False) without publishing a demand sample.
This atomically refuses an already active parent observation BEFORE body and prevents
mistaking an enclosing eager observation for a child's measured demand. Native eager
phase-overlap refusal chooses the ordinary eager/intent path once with an explicit
pre-body no-automatic reason; it does not clear/relabel the parent's observation.
Unexpected active-Plan/native errors propagate before body. No other native exception
is caught under that overlap case. If a preparation token cannot retire, retain the
execution owner and fail closed; never start a replacement body. No sample is promised
for capture/replay merely by holding this metadata-only preparation observation.

## Cache ownership and native pressure

Private graph pools/static Working remain charged while cached. Automatic mode must
not accumulate one immortal private pool per shape or treat weight Pins0 as zero cache
cost. One current key/entry per selected boundary bounds signature cardinality; old
in-flight entries cannot be discarded or replaced until checked tails permit retirement.
If that entry is still in flight, decide eager/no-new-capture before the new body;
do not append an unbounded retirement list or force completion/owner release.

Register each boundary cache with existing `_runtime.CacheOwner` (retaining its actual
weak callback target). Native touch/suspend selects/requestability; the callback's bytes
argument is intent, never a reclaimed-byte credit. No Python byte threshold, victim
score or app OOM retry. On a native shrink request retire that entry only when idle and
its last consumer tail is checked complete. Native ready-only trim owns physical release
and accounting after logical graph retirement; callback return is not physical credit.

Concrete required concurrency seam: callback takes ONLY the entry/admission lock
nonblocking, never the offload-wide execution lock held by an allocating active segment.
Otherwise idle pools would be unreclaimable exactly when another segment needs memory.
The controller must retain an entry admission lease across selection/prepare/replay;
callback declines active/in-flight/uncertain entries and never resets another epoch.
Add a private cached-entry retirement path on GraphSegment, under that entry lease,
which resets only its executable/footprint and closes only its inactive owned Plan/refs.
It must not read/rebind model state or deactivate any external/current segment Plan.
Existing native CacheOwner producer/external-callback exclusion and checked tails guard
this release. Any Busy/capture refusal retains the exact retryable entry; uncertainty
retains owners and explicitly fails the controller. No automatic retry of computation.

No shared pool is silently introduced. Ordered shared-pool execution remains separate
work; independent outputs and pool tail lifetime remain unchanged. Retiring cached
definition refs does not close shared Region wrappers or escaped weight/output owners.
Owner.close disables admission, drains CacheOwner outside device/entry locks, then
retires its idle definitions/measurement keys; refusal keeps retryable exact ownership.

## Minimal actual clients and qualification boundary

- CPU: real module/config/input keys, boundary frontier/nesting, rejection decisions,
  opaque Tensor non-retention, dtype/alias identity and eager mode parity. No fake
  native counters/Plans/allocators to claim automatic CUDA behavior.
- GPU: ordinary vt.offload + two declared blocks, no owner.capture/reserve calls;
  first eager native sample, subsequent automatically selected cached replays, exact
  parity and independent outputs, multiple definitions alive with weight Pins0,
  same-VA eviction/refill, changed shape/dtype/Conv configuration and known parent intent.
- Pressure: actual native CacheOwner request while another segment executes/allocates;
  idle dedicated pool retires through its own lock, active/in-flight pool refuses,
  measured mapping falls only after physical trim, cache rebuild remains exact.
- Failure: no authored body replay after any capture/launch/capacity failure; parent
  Plan/phase is handled before body or failed closed, with exact owners preserved.

No GPU/rental/machine/merge/release action is part of this contract delivery. Dedicated
pool pressure, key readiness and real capture/consumer failure cases must pass on the
coordinator's matched cohort before claiming automatic generic model graph support.
