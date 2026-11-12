# Varena: one authority for a process's GPU memory

Varena ships inside the TensorFS wheel and has no release of its own. It is an original
implementation against the CUDA Driver API. Using it: [PUBLIC-API.md](PUBLIC-API.md) for C, Rust
and Python, [TORCH-OFFLOAD.md](TORCH-OFFLOAD.md) for Diffusers.

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
5. idle peer processes, in no request (physical shortage only): what they give back, then, in a
   request, an idle one TensorD ends, oldest use first, for what no ask returns (its context, a
   graph's pinned weights)
6. completed idle weights in eviction rank, passing over ones still in use; short of those, waiting for the first of them to finish
7. a context join, for releases with unknown consumers
8. owner caches
9. peer processes (physical shortage only, one last retry on their final answer)
10. idle high-priority regions, once `Yes` has asked owners and peers

`Wait::No` takes completed memory only, `Local` also waits for submitted GPU work, and `Yes` adds
owners and peers under a 2 s budget of last resort. Eviction rank: contentless, prefetched and
unused, unplanned, later phases furthest first, then this phase's used regions most recent first
(those this pass filled before those that stayed through it, so a loop streams the same regions
each pass), then its next use. A planned loop keeps a prefix of its plan: a region it refilled
inside the prefix counts as kept, and the kept go from the prefix's end. A fill ahead takes only
the streamed regions. A plan phase that
ran through last time with no region coming back is a single pass: its used regions rank with the
unplanned, and other phases' loops go after every single pass's.

## Fills

A Region has a `Source`, and only the core calls it. `use` on an absent Region runs it on the caller's
thread, with no lock held and never during capture. One worker per GPU runs it ahead of demand, one
Region at a time: what comes next in the observed order, else the Plan's. Past the one Region a use asked
for, it fills only room already free. Demand stops it, and so does a shortfall, before anything real gives
way. A Source that needs its caller, for the GIL or a GPU kernel, says so in `ahead()` and never runs there.

TensorFS implements `Source`: a Region's page in host RAM, else the store, then its derive. It owns reads
and the page tier. Its decode scratch still comes from here.

## Completion

A free, unpin or replay stamps `(stream, sequence)` with no driver call. Outside capture, `poll`
seals one pooled event per dirty stream and queries them in order. Events are never destroyed, so
a waiter outside every lock holds a valid handle. A stream's owner calls `retire_stream` after
draining it; a release whose consumers are unknown takes a join mark instead.

A per-device gate excludes capture entry. It is held across short driver calls, never a GPU wait.
Lock order: gate, device table, fences, region waits. A producer guard held through a wait keeps
capture out; replay does not block reclaim, because pins and references protect what it uses.

## Working memory

Up to 1 MiB, one-class slabs, every block 512 B aligned. Above that, runs, cut best-fit and merged
when free; up to 64 MiB they share 64 MiB segments. A segment has one VMM handle per 16 MiB, since a
handle comes back only whole: free pages stay mapped until something needs them, and then only those
handles go. Own-stream reuse does no driver work. A plan phase seen looping keeps every segment its
working phases touched, up to its measured need: its weights and its own allocations take weights
first, and its allocations never wait on its own frees, so a step makes no driver call for working
memory. A weight's demand that would wait on another weight's use waits on the loop's frees instead,
and takes the pages past that need: a weight goes only after them.

A split (`split_select`, `split_begin`) picks the first level that fits
`min(max(C − W, C/4), C − live − pinned)`, whatever is free right now. `C` is the card or its cap,
`W` the phase's weights, `live` the live working bytes and `pinned` the most weights held at once
this request. A level's size is what it was measured asking for, or its nearest measured sibling's
ratio at 125%. Once a level has measured, one call runs the next coarser one if it never ran and the
card could hold it, since an estimate can stand well over a level that fits. That is once per site
and level per process, whatever comes of it; then the same request picks the same level.

## Graph pools

`vt.graph` prepares a pool, arms a small eager slab for auxiliary streams, and refuses a capture it
did not prepare. Torch's cached library workspaces live in no pool. An owner that keeps nothing in
a pool between replays declares that with `vt.pool_scratch(pool, held)`, which refuses an escaped
alias, and brackets each use with `vt.prepare_pool(pool, 0)` .. `vt.record_pool_stream(pool, stream)`.
Idle declared pools are then unmapped, keeping their addresses and graphs. Undeclared pools stay.
An owner done with a pool, every graph of it reset, says so with `vt.pool_close(pool)`: its pages go
at the next trim once its blocks are free, even after a capture whose end failed.

## Prefetch

A phase with a measure admits a prefetch when `free >= bytes + (last peak - what it already holds)`,
mapped reusable pages covering the gap first. It takes only idle completed memory, and what it
filled and nobody used is evicted first. A phase with no measure yet prefetches only into room
already free beside the `C/4` floor, and evicts nothing.

## Sharing a GPU

Each process takes a row in the GPU's ledger (`crates/varena-ledger`, Linux): one page of atomics.
An asker publishes `want`, each donor thread works out its own share and trims, and the asker
retries the driver. Rows are advisory: a table that cannot open costs donations, never an
allocation. Every participant donates in every mode.

Shared mode also yields to foreign programs. A process holding idle bytes samples the card every
500 ms: `free < H` asks for `2H - free`, where `H` is `max(total/16, 512 MiB)`, and a new foreign
process crossing 256 MiB asks for everything. Prefetch stops until a pass boundary with `2H` free.

## Caps

`vt.set_limit(device, bytes)` makes the card read as a smaller one, the driver's own charge
included.

## One process

Varena state and its CUDA objects belong to one host process. Threads are fine. A fork child is
refused before inherited locks, including raw forks, through a read-only `MADV_WIPEONFORK` page
armed before CUDA. A spawn helper sharing the parent's address space must exec or exit before
calling varena; the default guard doesn't catch one that copies authenticated TLS. The
`strict-pid-guard` Cargo feature checks the real PID on every call instead, and non-Linux targets
and a failed page setup do too.

## Tests

Real runs only: `release-gate/suites/varena/core` through `cozy run`, the model cells in
`release-gate/suites/varena/cells.sh`, and the deliberate faults in `faults.sh`.
