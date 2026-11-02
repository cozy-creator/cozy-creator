# P3 ledger foundation

This checkpoint adds Linux shared metadata only. It does not activate allocation policy, query CUDA, reclaim
GPU memory or delete TensorD's current room/lifecycle mechanism. General Varena remains independent of TensorD.
The P2 allocator/replay foundation is a dependency, not a new P3 qualification claim.

The first supported cooperation domain is one effective UID, user namespace, PID namespace, mount namespace
and host boot. Callers explicitly share a tmpfs backing directory and supply the actual physical GPU/MIG UUID.
The file key never uses a CUDA ordinal. The fixed, versioned header rejects incompatible identity/layout;
separate backing inodes are separate domains and are never claimed to cooperate. Different-user/private-PID
container support and CUDA/MIG UUID discovery remain open.

Each participant owns a bounded row incarnation and an independent OFD row lock. Every connection has an
independent short publication lock and local thread serialization. A process cannot reuse an inherited handle.
A bounded atfork registry closes only inherited ledger descriptors in the child, without unlocking the parent's
OFD description or running Python/allocating. Descriptor state is invalidated before any later Rust destructor,
so a reused numeric descriptor cannot be closed twice. CLOEXEC handles exec. Exact process birth supplements
lock ownership; elapsed time and an unchanged heartbeat never mean death. Reaping metadata credits no GPU bytes.

Call metadata-only `ledger::initialize()` before concurrent fork producers start. The first initialization
(including an implicit first `open`) must not race a fork already in progress: that fork may already have read
the old libc handler list. This checkpoint deliberately supplies no general fork interposer. After initialization,
concurrent ledger open/close and libc/Python fork are supported and exercised. A raw syscall fork/clone that
bypasses registered libc handlers remains outside this inheritance contract; it must not be claimed qualified.
The installed native hook lives for the process lifetime; an embedding must keep this library loaded afterward.
Snapshots are last committed metadata, not a promise that each owner is still alive. Reaping and ask creation
perform the exact row-lock/birth checks; neither operation adds dead owners' holdings to available GPU bytes.

The table uses two metadata banks with an atomic committed-bank generation. A writer finishes the inactive
bank before publishing its index; a process killed while writing leaves the last committed bank readable.
Snapshots are taken under the short publication lock. No user callback, CUDA operation or wait for a donor
runs under that lock.
The fixed tmpfs backing is allocated before mmap on every compatible open, including retries after an ENOSPC
failure left a correctly sized sparse file. Existing nonzero headers are checked before allocation; no
incompatible layout is repaired. This makes a small/full Docker shm limit an ordinary I/O refusal, not SIGBUS.

Rows publish disjoint physical weights, nonpool working storage, graph-pool storage and owner-rebuildable cache
classes. Live/idle/pending subdivisions partition each class. Reclaimable bytes are a subset of idle holdings,
not another physical category; graph-pool bytes must not also be included in nonpool working bytes. Numbers
are caller observations in this foundation, not GPU evidence.

One ask round binds its sample generation and donor row incarnations. Concurrent asks join it; repeated donor
acknowledgements replace cumulative progress rather than add it. Pending work and refusal are distinct from
reported completed physical release. The requester must re-read actual driver capacity after a round; the table
never grants/reserves memory or transfers CUDA handles. Row death does not turn a request into returned bytes.

Capacity observations use an optimistic commit: obtain `Ledger::snapshot().generation`, observe the driver
outside every ledger lock, then pass that expected generation to `Participant::sample`. An intervening metadata
publication returns `Error::StaleSample`; obtain a fresh snapshot **and** observation instead of reusing the old
reading. The sample records a capacity epoch. Every physical publication (even identical final counts), row
membership change, or increase in completed physical release advances that epoch. A sample predating one of
those changes cannot open a new ask round. It can still join an unfinished round or retrieve the already answered
round for its own generation: neither spends the deficit again. Reaping invalidates an observation without
crediting the dead row's holdings as released memory. Dead-row cleanup commits before ask validation so a stale
sample cannot roll back the reap and force an endless retry.

Exact duplicate acknowledgements change neither the capacity epoch nor the publication generation; identical
metadata transactions do not rewrite a bank. Pending acknowledgements do not advance capacity until completed
bytes increase. This prevents duplicate delivery from starving an optimistic sample commit. Layout version 2
stores both epochs in the committed bank; older layouts are refused, never resized or repaired. This validates
observations against published metadata, not unreported physical transitions or concurrent foreign CUDA changes;
the actual driver allocation remains the authority. No sampling leader, daemon, grant or CUDA call is added.

Real CPU proof must use independent processes, kernel-held locks, fork and explicit process death: publication,
concurrent asks/duplicate acks, stale row identities, a stopped-but-live owner, killed writer recovery, parent death
while its forked child survives, and descriptor-number reuse before dropping inherited Rust objects. Namespace
proof is reported only when an actual namespace transition can run; a fabricated namespace ID is not that proof.
No CUDA test, foreign-game latency or complete P3 acceptance is implied by these CPU checks.


The separate [GPU integration checkpoint](cooperative-gpu.md) adds an explicit native participant
on top of this metadata API. It does not change the metadata-only CPU proof into GPU evidence or
activate TensorD/allocator policy automatically.

## Automatic-owner connection mode

`Ledger::open_nonblocking` is the GPU participant's connection. Its operations
try the process-local serialization mutex and OFD publication lock, returning
`Busy` on contention. A live writer stopped while holding publication must not
trap a donor service tick or its owner's shutdown. Existing `Ledger::open`
retains its blocking operation contract for explicit CPU callers.

Participant destruction now always clears its row on a best-effort nonblocking
basis, then releases the independent row descriptor. A read-only snapshot can
therefore retain departed metadata. A coordinator explicitly commits reaping
before a fresh capacity sample; an available row lock permits exact reaping even
while the former owner process stays alive. Row disappearance never counts as
GPU release and never changes an observed driver-capacity sample. GPU `Peer`
open, service and sample perform that separate reaping; asks already did so.

Real stopped-holder and live-owner-close proof is in
`release-gate/suites/varena/ledger-cpu/nonblocking.py`. The fork/open race example
also verifies departed row lock availability after exact child waits before
reaping and checking its strict one-live-row invariant.
