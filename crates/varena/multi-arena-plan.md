# Device-wide multi-arena Plan checkpoint

Owner: `/root/p3_owner_restart`, branch `codex/xs138-p3-multi-arena-plan`.
Fetched base: `1b77958bdfb3f699db8340951c9b5cc1805d4243`.
Coordinator baseline: `106581cb66f161a218672032ebcbc0b59b89dd01`.
Clean integration: `b4a5b30176d7924611bc5a54491f537643aca0a5`, identical to coordinator
product source. Frozen 065/6c812 and shared-service db701 remain unchanged.

## Proposed API for coordinator review

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
wrap/reuse, and no address-derived identity is exposed. Fork remains unsupported.

Device inventory includes publication and logical retirement of both arenas and
regions. Every such mutation invalidates admission, even without an active Plan.
Existing selected `plan_generation` therefore cannot survive an inventory change.
Generation exhaustion fails closed; an invalidation epoch is not a capacity grant.

Strong Plan/Arena leases are acquired before capture/arena/working guards. Arena
registrations carry stable IDs. A registered arena whose weak owner cannot upgrade
is retiring, not an ignorable hole in a supposedly stable inventory. Its destructor
removes publication by numeric ID inside capture exclusion. Region retirement also
linearizes its Retired marker there. Plan snapshots and reclaim take one arena
mutex at a time in stable Arena-ID order, under capture exclusion, and release all
guards before owning leases can be destroyed.

The active registry still permits one plan/phase per device. Multi-arena phase bytes
and missing backing are deduplicated by stable Region identity. Completed unpinned
outside-phase backing remains eligible for native reclaim; queues, VA and unknown
completion never supply bytes. No per-package memory policy or application replay
is added.

## Proof to stage

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
