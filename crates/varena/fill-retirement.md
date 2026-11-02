# Fill and Arena retirement integration

Owner: /root/async_integration_review. Branch codex/xs138-p4-fill-retirement starts from
fetched origin/master551980610 and composes coordinator7a653de29 with checked async8b77294bb.
Native Arena retirement49685f087 and current-format ledger2ed2d1926 are integrated.
No external P4 worktree is edited here. This is a draft integration, not GPU qualification
or release.

The native topology is one event-polling worker and one typed retirement queue per
device. The queue owns Arena finalization and checked Fill resources. A submitted Fill
retains every fixed producer stream, source/staging resource, producer Pin and exclusion
lease. Published data is available only to explicitly ordered consumers; Ready follows
checked producer completion. Pending resources remain owned without blocking later jobs.

The authored, unrun combined GPU fixture requests Arena retirement while a Fill and separately gated
ordered consumer retain real ownership. Another Arena must continue useful work. Producer
completion alone must not free consumer-backed bytes, and last-consumer completion must
permit exact final physical retirement without a context/device synchronize fallback.
The existing five standalone async modes remain intact. General TensorFS Source adoption,
public Source adapters, GPU execution and rental operation stay outside this follow-up.

## Native contracts

The public signatures remain the checked Fill interface from8b and the Arena interface
from49685. `FillOwners` contains a fixed list of producer streams and checked FillResource
owners; Session begin/publish/abort and Ticket facts keep their existing contracts.
`Arena::request_retirement(self: Arc<Self>) -> Result<ArenaRetirement, (Arc<Self>, Error)>`
returns a metadata-only ticket with facts/try_complete. No adapter receives a byte-policy
callback or permission to discard real owners.

The private queue is `retirement::Job::{Arena, FillResources}`. Producer completion calls
`retirement::defer(device, Job::FillResources(payload))`; both worker endpoints exist before
destination exposure. Pending payloads stay queued, permanent errors retain exact remaining
owners in the shared quarantine, and the event poller never performs resource destruction.
A successful Fill ticket retirement means its native payload retired; a final Arena job
may still be waiting for consumer fences. Only actual physical release changes capacity.

The merge also routes both asynchronous driver calls through the retained counter metadata
and excludes Published from Plan outside-phase candidates. Global region identities and
complete inventory epochs remain the coordinator's multi-arena implementation.

## Validation boundary

Native library and the complete async example compile together. The original five GPU
cases are unchanged; `arena_retirement` is the sixth authored mode. No mode has executed
on a GPU for this cohort. Binding compilation and source freeze receipts are recorded by
the owning agent under outputs/xs138-p4-fill-retirement, then linked in the draft PR.
