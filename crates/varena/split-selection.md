# Closed leaf split selection

This first P2 consumer boundary owns no tensor geometry or execution loop. The consumer copies
an ordered menu of `(opaque choice ID, simultaneous new logical allocation requests)` into
Varena. The full site key identifies exact input layout/dtype, output contract, kernel artifact,
dispatch and legal geometry. A changed menu under the same live key is rejected.

- `split_context(device)` reports only actual metadata eligibility: capture, replay, another
  leaf scope or a non-eager enclosing phase excludes the optional adapter. It does not initialize
  CUDA, query capacity or expose a second Python memory policy.
- `split_select` derives physical demand with Varena's own size classes and exact per-class slab
  slot counts. It first chooses the largest legal entry fitting while preserving weights, then
  reclaims idle working backing, then idle weights toward the declared efficient choice. A
  successful sample replaces only that exact site's choice declaration, never an unseen chunk.
- `split_begin` revalidates the agreed ID and opens one leaf interval. It never chooses another
  ID after commitment. The optional completion owner must be the active eager phase; the consumer
  passes it only when that owner's actual checked stream covers this leaf's launches.
- `split_end(submitted=...)` closes host submission, not GPU completion. The parent phase's
  existing successful completion publishes pending exact-choice samples. Abort, allocation
  failure, retry, callback memory, capture/replay or an open leaf interval discard them. An
  unknown parent phase scheduling key does not erase a valid leaf completion proof. Outside an
  explicit owner there is no learned sample; peak fields are reported unknown.

New allocation addresses are attributed only within the active leaf. Existing outside inputs
and outputs remain global charges. Live/pending block capacity and distinct physical backing
are measured separately. Leaf observation hooks execute only inside the existing active-phase
path; no new per-block tag or inactive allocation syscall is introduced. There is no per-norm
or per-chunk synchronization/event. Native reclaim may perform the existing completion work
only when capacity is needed.

`phase_forget(site_key)` retires the copied menu and samples with its Model/Request owner, and
refuses while its leaf or pending completion is active. Native `Exhausted` reports required and
observed available bytes before invoking the operation; Python maps it to Torch OOM with
`varena_no_retry=True`. A consumer also marks an unexpected post-begin allocation OOM before
propagating it, because earlier chunks may already have written private output. Allocation
recovery can retry an allocation, never an operation or an outer package.

The first actual adapter is noncontiguous, batched inference channel_norm with a known compiled
launcher and complete full-output plus one equal-divisor copy declaration. Contiguous inputs,
unsupported/nested/capture/collective contexts and opaque fallback kernels remain outside that
adapter. Capture uses the existing whole implementation until an actual precommitted capture
choice exists. This is not complete P2 or qualification of arbitrary author.split, convolution,
INT4/INT8 workspace guesses, H3 fit.py deletion or cross-rank selection.

## Stream-aware allocation geometry

Both select and begin require the actual raw execution stream. Each choice matches its copied
allocation classes against UNIQUE free blocks in the allocator's existing take order, using
the same reuse predicate. A globally completed block or a block safely ordered on that stream
can satisfy one request. Live, unjoined, multi-stream-pending and graph-private blocks cannot;
pending bytes on another stream are not generic headroom.

Learned choices retain measured per-class peak counts alongside scalar block/backing peaks.
The first closed full-output-plus-copy family has a simultaneous class envelope. For a more
complex sequential family, a per-class envelope may be conservative; it is not a fitted
cross-choice estimate or a claim that different class peaks necessarily occurred together.
Scalar peaks remain historical diagnostics rather than pretending every existing byte can
serve every allocation class.

`required_bytes` now reports additional backing for unmatched requests;
`allocation_backing_bytes` is the fresh backing cost of the complete class envelope.
`reused_capacity_bytes` counts matched block capacity once. `preserved_backing_bytes` describes
the segments that contain those slots, including any other live occupants; it is not added to
free capacity. `reclaimable_working_bytes` covers only unrelated completed whole eager segments.
Select reports feasibility using that reclaimable backing; begin re-evaluates and performs any
necessary release before execution while preserving matched segments. Compatible cache is
not unmapped merely because the total fresh allocation cost exceeds driver-free memory.

The ordinary allocation slow path's unconditional weight reclaim after working trim remains
a separate unified-reclaim followup; this correction does not silently alter it.
