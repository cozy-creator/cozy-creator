# Varena: one authority for a process's GPU memory

Varena ships inside the TensorFS wheel and has no release of its own. It is an original
implementation against the CUDA Driver API.

## Who decides

One `Device` per GPU holds one table of every byte this process has on the card: weight regions,
working blocks, graph pools and owner caches. Only `reclaim`'s one sequence gives bytes up.

Everyone else allocates, pins for use, declares intent (`plan`, `phase`, legal splits) and reads
stats. The two exceptions act only when the core asks: the owner of a rebuildable cache releases
what it is asked for (`CacheOwner`), and TensorD may end an idle process.

## Rules that never bend

- Never unmap memory a kernel may still use.
- No driver call during capture: allocation and free are metadata on prepared backing.
- Nothing waits on the GIL or a lock its caller holds; no lock is held across a GPU wait.

## Reclaim

`reclaim::acquire(device, need, wait, protect, try_map)` retries `try_map` and walks one order:

1. completed free working pages
2. contentless regions
3. idle graph pools (a remap, no re-read)
4. working frees still running: wait
5. idle peer processes, in no request (physical shortage only)
6. idle weights, strictly in eviction rank, waiting for one still in use rather than passing it
7. a context join, for releases with unknown consumers
8. owner caches
9. peer processes (physical shortage only, one last retry on their final answer)

`Wait::No` stops before any wait, `Local` before other parties, `Yes` runs it all under a bounded
budget of last resort. Eviction rank: contentless, prefetched and unused, unplanned, later phases
furthest first, then this phase's used regions most recent first, then its next use.

## Completion

A free, unpin or replay stamps `(stream, sequence)` with no driver call. Outside capture, `poll`
seals one pooled event per dirty stream and queries them in order. Events are never destroyed, so
a waiter outside every lock holds a valid handle. A stream's owner calls `retire_stream` after
draining it; a release whose consumers are unknown takes a join mark instead.

A per-device gate excludes capture entry. It is held across short driver calls, never a GPU wait.
Lock order: gate, device table, fences, region waits. A producer guard held through a wait keeps
capture out; replay does not block reclaim, because pins and references protect what it uses.

## Working memory

Small size classes are 512-byte aligned. Larger blocks are runs in segments of at least 64 MiB, cut
best-fit and merged when free; a segment returns its pages once it is all free. Own-stream reuse does
no driver work. A split (`split_select`, `split_begin`) caps each operation at
`max(capacity - phase_bytes, capacity / 5)`, and at what live working bytes and the most weights
held this request leave, whatever is free right now. A level's size is what it was measured asking
for, or its nearest measured sibling's ratio, so the same request picks the same level.

## Graph pools

`vt.graph` prepares a pool, arms a small eager slab for auxiliary streams, and refuses a capture it
did not prepare. Torch's cached library workspaces live in no pool. An owner that keeps nothing in
a pool between replays declares that with `vt.pool_scratch(pool, held)`, which refuses an escaped
alias, and brackets each use with `vt.prepare_pool(pool, 0)` .. `vt.record_pool_stream(pool, stream)`.
Idle declared pools are then unmapped, keeping their addresses and graphs. Undeclared pools stay.

## Prefetch

A prefetch is admitted when `free >= bytes + (last peak for this phase - current backing)`, mapped
reusable pages covering the gap first. It takes only idle completed memory, and what it filled and
nobody used is evicted first.

## Caps

`vt.set_limit(device, bytes)` makes the card read as a smaller one, the driver's own charge
included; `make_room(device, limit)` holds this process's own bytes to a limit for foreign pressure.

## Tests

Real runs only: `release-gate/suites/varena/core` through `cozy run`, the model cells in
`release-gate/suites/varena/cells.sh`. See also `process-contract.md` and `PUBLIC-API.md`.
