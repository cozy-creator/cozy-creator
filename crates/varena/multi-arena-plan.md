# Device-wide multi-arena Plan checkpoint

Owner: `/root/p3_owner_restart`, branch `codex/xs138-p3-multi-arena-plan`.
Fetched base: `1b77958bdfb3f699db8340951c9b5cc1805d4243`.
Coordinator baseline: `106581cb66f161a218672032ebcbc0b59b89dd01`.
Clean integration: `b4a5b30176d7924611bc5a54491f537643aca0a5`, identical to coordinator
product source. Frozen 065/6c812 and shared-service db701 remain unchanged.

## Implemented API for review before cohort adoption

```rust
Plan::new(anchor: Arc<Arena>, definitions: Vec<(String, Vec<Arc<Region>>)>) -> Result<Arc<Plan>>;
Arena::id(&self) -> u64;
Region::id(&self) -> u64;
Region::arena_id(&self) -> u64;
plan::inventory_generation(device: i32) -> Result<u64>;
```

The constructor signature stays unchanged. References may span every arena on
the anchor's device; a foreign-device region refuses before activation or writes.
One checked process-wide identity sequence supplies Arena and Region identities
and private Plan identity. Zero is invalid, overflow refuses, identities never
wrap/reuse, and no address-derived lifetime identity is exposed. These IDs remain
out of compute fingerprints. Fork remains unsupported. Creator/TensorFS versions
are unchanged; this source is not a new released or GPU-qualified cohort.

Device inventory includes publication and logical retirement of both arenas and
regions. Every such mutation invalidates admission, even without an active Plan.
Existing selected `plan_generation` therefore cannot survive an inventory change.
Generation exhaustion latches the epoch unusable even when a destructor ignores
its error. An invalidation epoch is not a capacity grant, and it may advance before
an ultimately failed publication. Committed entry rechecks its expected epoch under
capture exclusion immediately before creating the working phase; split begin already
revalidates under that guard before writes.

Strong Plan/Arena leases are acquired before capture/arena/working guards. Arena
registrations carry stable IDs. A registered arena whose weak owner cannot upgrade
is retiring, not an ignorable hole in a supposedly stable inventory. Its destructor
removes publication by numeric ID inside capture exclusion. Region retirement also
linearizes its Retired marker there. Plan snapshots and reclaim take one arena
mutex at a time in stable Arena-ID order, under capture exclusion, and release all
guards before owning leases can be destroyed. Native policy visits stable Arena IDs
and then existing per-arena eviction keys; it does not compare independent clocks
as a purported global LRU.

The Python release handoff calls `Region::retire` before its deferred cleanup worker.
That handoff only tries capture exclusion: if another native teardown owns it, the
complete Region stays in RetiredRegion until deferred Drop can serialize retirement.
It does not hold the GIL while waiting behind a GPU synchronization.

The active registry still permits one plan/phase per device. Multi-arena phase bytes
and missing backing are deduplicated by stable Region identity. Completed unpinned
outside-phase backing remains eligible for native reclaim; queues, VA and unknown
completion never supply bytes. No per-package memory policy or application replay
is added.

## Staged proof and qualification boundary

A genuine GPU fixture must use at least two separately created arenas with phase
regions from both, duplicate the same region in input to verify exact deduplication,
and retain unrelated outside-phase backing on another arena. It must verify actual
missing bytes and outside reclamation/readback, cross-device refusal when two GPUs
are available, one-active-plan exclusion, and invalidate selected/committed plans
on region and arena publication/retirement. A concurrent publication/retirement arm
must allow typed deferral but never accept stale intent or drop final owners under
allocator/registry guards. Preserve zero-new-driver-call capture refusal and exact
task-owned final accounting.

CPU checks may qualify identity uniqueness/exhaustion and host inventory epochs
using actual native synchronization. They do not qualify CUDA allocation, GPU
retirement, model behavior or the staged multi-arena fixture. Root owns all GPU and
portable build actions; no heavy CPU job starts without coordination.


The source/binding check passed at one nice19 Cargo job (65 seconds). The native
GPU helper type-check passed, including a locked warm recheck (0.36 seconds).
The actual host proof passed:8,192 concurrent identities,512 inventory transitions,
inactive-intent invalidation, per-device isolation, both overflow latches, and fork
refusal before the inherited registry mutex. No CUDA/NVML mapping was present.
Receipts are under `release-gate/suites/varena/evidence/multi-arena-plan-cpu` and
workspace `outputs/xs138-p3-multi-arena-plan/host-001`.

The new helper cases are `multi_arena_plan` and `multi_arena_plan_cross_device`.
The first uses three real arenas, alias deduplication, missing-byte accounting,
managed-cap forced outside reclaim, full planned/output/refill readback, all four
stale inventory transitions, safe Arc::try_unwrap retirement gap, strong Plan
retention,32 concurrent arena/region lifetimes and actual CUDA capture refusal.
The second requires two actual leased GPUs and refuses a foreign-device region.
They are compiled source only: neither GPU case has run.

Coordinator106581 uses a shared-only native core. The helper must be built/bundled
with its exact core and matching Rust runtime; the historical lone-binary prepare
path is insufficient. No second static core is allowed. Source stage is therefore
NOT portable-payload readiness. The helper-only Cargo.lock gained15 cached NVML
transitive dependencies required by this coordinator baseline; workspace product
Cargo dependencies did not change. Freeze with `--native-base 106581cb66f161a218672032ebcbc0b59b89dd01`;
prepare the two-GPU case with `--expected-devices 2` and ordinary Cozy `--gpus=2`.
External P4 packaging and all GPU execution remain with their assigned owners.
