# Explicit native GPU cooperation (P3 checkpoint)

This connects the Linux metadata foundation to actual Varena GPU ownership. It installs no
allocator callback, background sampler, process byte target, TensorD controller or Python cache
callback. Source/build evidence and real GPU qualification are separate; the GPU fixture is
prepared here, not claimed executed. TensorD policy removal remains later work.

`cooperative::Peer::open(device, typed_uuid, directory, sharing, priority)` initializes the
native working device outside capture and verifies the UUID bytes with `cuDeviceGetUuid_v2`.
The caller supplies the GPU/MIG kind; automatic kind discovery and MIG execution remain
unqualified. A missing/wrong descriptor fails, never silently creates a private cooperation
domain. Same effective UID, user/PID/mount namespaces, boot and shared tmpfs backing are still
required. Call `ledger::initialize` before concurrent fork producers, as in the foundation.

The Python adapter is `tensorfs.varena.Cooperation`. `initialize()` installs only fork handling;
construct it in the fresh CUDA child, after installing the Torch allocator. Its explicit methods
are `publish(used=False)`, `sample()`, `ask(sample_generation, wanted)`, `service()`, `snapshot()`
and `close()`. This is a staged adapter, not the final P4 `Gpu` API. Samples with stale metadata
raise an error and must be observed afresh. H and per-PID attribution are unknown in this slice.

`publish` takes native state, not caller-supplied byte counts. Nonpool working and graph-pool
segments are disjoint; block capacities and unassigned tails partition every mapped segment.
Weight mappings are counted once. `caches=0`: cache tensors already belong to working storage,
and graph pools are not charged a second time. Reclaimable bytes include only complete whole
mappings. Live blocks, pins, loading regions, graph references and partial slabs remain held.
Unknown consumers are pending physical holdings, but not a promise of event-backed release.

A refresh may query/record known completion events under native capture exclusion and local
allocator locks. It never joins unknown consumers or waits for an event. During capture or the
host replay guard it copies metadata only (`refreshed=false`), before any UUID/context/query
operation. The native driver activity counter includes initialized-driver context push/pop,
memory sampling, event and VMM calls; reading the counter performs no CUDA operation.

Lock/effect boundaries:

1. Copy the donor assignment under the ledger lock and release it.
2. Capture exclusion precedes working state and arena states. Refresh without joining; use
   existing completed working trim and weight eviction order/primitives. There is no callback.
3. Record each successful unmap+release immediately, before fallible event/address cleanup or
   a later coherent snapshot. Persist cumulative local donation progress even if that snapshot
   or publication fails. A retry is capped by the old donor snapshot minus completed effects.
4. Release native allocator, context and capture guards before publishing or acknowledging.
   Never hold the shared table lock across CUDA, an allocator lock, Python or an owner wait.

The physical partition must equal the native managed-mapping counters. A final arena destructor
already past its last strong reference may make a temporary coherent snapshot unavailable;
return Busy without publishing guessed counts. Any effects already completed survive that error.
Driver errors remain latched at their working/arena owner. An error does not justify retrying an
unmap/release whose ownership is uncertain.

An ack reports actual release, not a logical free or promised capacity. No sampler election,
lease, allocation grant, transferred GPU handle or whole-request retry is introduced. The
requester still attempts its real driver allocation. Row death credits no returned bytes.

## Retained arenas and close

A participant retains every arena observed on its device for its explicit lifetime. Without
this ownership, dropping a temporary snapshot Arc could become final destruction and context
synchronize inside a supposedly nonblocking service tick. The registry carries device identity
beside Weak references so a tick never upgrades/drops an unrelated device's last owner.
Regions still retire and release their actual physical backing normally. Service does not
promise to destroy every arena's metadata.

`Peer::retire` acquires a short retirement exclusion and returns an owning `Retirement`. A
refusal returns the complete Peer intact. Drop Retirement outside application locks: its field
order drops the Peer and retained arenas before releasing the exclusion. The exclusion rejects
new capture/replay but allows ordinary teardown and holds no capture mutex across Arena::drop.
`try_close` performs this sequence directly. Raw Rust Drop inside capture remains unqualified;
the eventual public Gpu API must use guarded retirement.

Python explicit close restores the intact owner when retirement is refused. It unlocks and
detaches the GIL before actual destruction. Automatic GC transfers ownership to the **existing**
TensorFS cleanup queue. CaptureActive requeues the intact owner using the existing Unready /
Abandoned pattern, so the worker can process other releases; no additional reaper/thread or
completion framework exists. Unknown fatal retirement state retains ownership rather than
claiming successful cleanup.

## Real GPU fixture

The ordinary Cozy `p0-working/cooperative` entry point starts two fresh exec children, installs
the exact candidate allocator, validates extension/shim/Torch identity, and joins one task-owned
tmpfs file using an explicit `gpu_uuid` payload field. The controller creates no CUDA context.
The fixture uses real TensorFS source files, actual Torch allocations and submitted GPU consumers.

It records actual driver observations and verifies:

- complete working/weight release, physical partitions and preserved live/pinned storage;
- a pre-release sample cannot open a second round; duplicate service cannot add release twice;
- pending stream consumers retain storage, no peer context/event waits, subsequent output and
  physical release after the owner completes its actual work;
- an unannotated native DLPack owner stays Unjoined; a peer does not context-join it;
- actual capture/replay service makes zero native driver calls; refused close retains ownership;
- external arena/region owner drops overlap a service while unrelated GPU work remains pending;
- Python GC during capture retains the row after a later cleanup sentinel completes, then
  releases managed backing and permits a fresh row incarnation after capture ends;
- a concurrent capture attempt during actual retirement reports whether exclusion was observed.
  If the existing teardown mutex serializes it until afterward, that race is explicitly
  unexercised, not reported as a passed concurrent-exclusion observation;
- death of the actual donor during an unfinished round credits zero; subsequent driver reads
  and successful allocations are evidence separate from row reap.

This is a mechanism fixture, not the 12GB foreign-game latency or SDXL/Qwen coexistence gate.
No namespace/container/MIG qualification follows from two same-domain processes. Exact native
byte/lifetime assertions remain strict. Graph cache owner retirement, notifications, autonomous
foreign-pressure sampling and TensorD cutover remain open.
