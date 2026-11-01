# Physical shortage recovery — source checkpoint

Working allocation and demand Region admission exhaust their local reclaim/retry
paths before asking the native owner service for peer progress. A private refusal
cause distinguishes actual `cuMemCreate` OOM from managed room, virtual-address and
streaming-window limits. Capture pools, prefetch and limited admission do not ask.
The already GPU-qualified bounded first working retry (`bda123e7b`, b20 artifact)
is included; pressure does not restore the old whole-cache flush.

Every native allocation episode gets one monotonic two-second coordination budget.
Busy/stale publications and unanswered/pending donors consume that same budget.
Joining a local wait or another round cannot reset it. Expiry records the typed
`peer_wait_budget_exhausted` outcome and permits one final local admission pass,
including its usual finite reclaim/driver retries. It is not necessarily one
`cuMemCreate` call. If that pass fails, ordinary exhaustion remains. This does not bound CUDA
event waits or total allocation latency, infer donor death, cancel another owner's
round, or turn acknowledgements into free capacity.

The native thread continues donating while its own process waits. Metadata commands
run in finite batches. The allocator retains only host control, releases every
capture/arena/working/context guard before waiting, and tracks observed round
completion so a duplicate observation does not cause another progress retry.
Phase invalidation and terminal working OOM counts happen after the whole episode,
so successful peer recovery does not invalidate a measured pass. Region retry
revalidates admission before publishing a Fill; no Source or package call is rerun.

`driver_physical_ooms` counts actual create refusals in working/Arena statistics.
Owner facts expose coordination attempts and the last typed result. Observed release
progress is advisory and is not a summable capacity grant across local waiters.

Current evidence: native checks plus the release-gate host-control harness. Its real
two-second wait ends with a final-retry instruction, preserves the same deadline,
leaves control active and holds no capture-registry mutex. It does not fabricate a
Peer, driver result or GPU release. The preceding service-only cohort remains
`1fec2950b`/Runtime `4d5844f0` (GPU runs 25190/25191).

Required next evidence: actual physical working/weight OOM and driver retry; local
limit/VA/prefetch negatives; phase validity after recovery; GIL-held Storage.resize_
and donor progress; overlapping requester/donor roles; real stopped/live/dead peers;
capture/retirement/fork; and a standalone native consumer. Cache callbacks and
foreign-pressure control remain outside this increment. No new GPU qualification
or release is claimed by this source checkpoint.

An allocation's recovery interest also has a lifetime. A successful local retry,
terminal coordination result, cancellation, or expired last local deadline queues
withdrawal of this participant's exact owned round. The native worker performs the
ledger transaction outside caller/control locks. A queued withdrawal stays queued
if another caller arrives; it is drained before a new ask. Other valid local
waiters retain their interest, and another requester's round cannot be withdrawn.
An ask that completes after its initiating wait expired is retired the same way.

The row's `want` is the most recently published current demand, not the sum of
local waiters and never a grant. Ending the last local demand clears only this
row's `want`, including when it joined a foreign round. Neither operation changes
holdings, completed-release counts, driver samples or capacity epochs.

The real three-process CPU regression preserves the pre-fix result: a stopped,
live donor A leaves round 9 Waiting, and a fresh ask still joins it despite newly
shared, higher-priority B. After exact owner withdrawal, a fresh round selects B.
Wrong-owner/wrong-generation/duplicate withdrawal and foreign demand clearing are
checked separately. This is a protocol proof with declared metadata bytes; the
Runtime `renewal` cell must still establish actual GPU recovery while A stays stopped.
