# Versioned activity and cache-owner coordination

Work in progress, not a portable/GPU candidate. This branch composes integrated
7e/a96, sampler27e, the9b98 ready-only collector and modef57 on fresh87b. The source
and GPU-qualified predecessors remain unchanged. Runtime/cache bridge composition,
the latest replay-collector correction and fresh GPU proof are still required.

The scoped filename stays the same, while the header version and layout advance to3.
Rows retain numeric consumer priority and add explicit Unknown/Idle/Active execution
activity plus requestability. Unknown sorts with Active; priority remains the first
ordering key. Requestability means an owner can be asked, never that any byte is free.
Native request boundaries publish activity; native cache registration supplies the
requestable fact. No Python byte target or polling policy is introduced.

An old table can be replaced only while publication and every independent row lock
are held. Live and stopped participants block migration. Stale rows grant no memory.
The new table is fully allocated/initialized first. Then the old selector becomes0
and its magic becomes a nonzero incompatible tombstone before atomic replacement.
This matters because v2 checked the named inode before taking publication: an old
waiter could otherwise register a row lock on the new inode while publishing into
the detached old mapping. Failed rename restores the old selector/header; an
interrupted tombstone is recoverable by a new opener. V3 checks identity again after
publication admission. H resets to unknown for the new ledger lifetime; new writers
preserve the maximum qualified measurement across unknown/lower observations.

Rounds distinguish Allocation, measured DevicePressure and AllIdle. AllIdle is one
new-foreign-process threshold episode, bounded by a fresh device-total observation,
not a measured allocation deficit or releasable-byte claim. It does not recur just
because that process remains alive. Its coordination deadline does not reset when
joining another round; expiry is recorded without inferring process death.

Known ready offers are assigned first. Unasked candidates retain their own snapshot
and cannot acknowledge an assignment prematurely. Once ready work is terminal, one
requestable owner-action stage is activated at a time. A ready donor may then enter
its own action stage for the remaining demand. A stage bit prevents an old ready
acknowledgement from closing a later action. Declines/exits advance with zero credit;
pending and requested amounts never reduce demand. Each donor's actual cumulative
release remains bounded by held-at-ask backing, not VA reservations.

The peer mints one process-unique cache RequestKey for its exact round, after the
capped native pass and outside Peer/capture/allocator/ledger guards. Callback/GIL
execution stays on the cache dispatcher. Dedicated configuration cancels the request
and returns Busy if queue admission or callback execution is still active; it never
waits for the GIL or acknowledges a new mode while that old action can start.
Synchronous cooperation close/service also refuses the callback's own dispatcher.

For Cooperative/Foreign, queue completion and the next callback amount use explicit
cumulative capped service. The global Room release epoch remains advisory only.
The peer snapshots action_epoch before its native pass and reports it with service
progress; a callback finishing during the pass therefore requires a later pass.
Repeated queue requests max-coalesce the target but cannot advance an action. Even
the final returned callback needs a matching post-action service pass before the
job finishes. Allocation-style requests retain their existing actual-retry signal.

CPU proof is real native threads/processes, OFD locks and actual filesystem faults.
The old/new metadata gate covers stopped owners, waiting old transactions/registers/
opens, the rename-only ghost-lock negative control, crash recovery and rename rollback.
Its declared byte facts are not GPU release. Host callback tests cover cancellation
and dispatcher self-wait refusal without loading CUDA/NVML. Scoped-progress epoch
tests and latest replay integration are being completed. No new GPU claim follows
from these CPU receipts.
