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

Rows publish disjoint physical weights, nonpool working storage, graph-pool storage and owner-rebuildable cache
classes. Live/idle/pending subdivisions partition each class. Reclaimable bytes are a subset of idle holdings,
not another physical category; graph-pool bytes must not also be included in nonpool working bytes. Numbers
are caller observations in this foundation, not GPU evidence.

One ask round binds its sample generation and donor row incarnations. Concurrent asks join it; repeated donor
acknowledgements replace cumulative progress rather than add it. Pending work and refusal are distinct from
reported completed physical release. The requester must re-read actual driver capacity after a round; the table
never grants/reserves memory or transfers CUDA handles. Row death does not turn a request into returned bytes.

Real CPU proof must use independent processes, kernel-held locks, fork and explicit process death: publication,
concurrent asks/duplicate acks, stale row identities, a stopped-but-live owner, killed writer recovery, parent death
while its forked child survives, and descriptor-number reuse before dropping inherited Rust objects. Namespace
proof is reported only when an actual namespace transition can run; a fabricated namespace ID is not that proof.
No CUDA test, foreign-game latency or complete P3 acceptance is implied by these CPU checks.
