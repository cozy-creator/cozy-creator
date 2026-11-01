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
