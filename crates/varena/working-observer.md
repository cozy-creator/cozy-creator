# Torch 2.15 Working segment observation

Run 26854 aborts during NCCL2 group construction. In Torch d0f2af8b66b20da4baa50644c0cd3f887a2eae2e, NCCL2 unconditionally reads the allocator snapshot and installs a segment trace listener. The stock pluggable allocator implements neither operation. Its destructor retries the same failed singleton construction and terminates the process. This correction keeps the current NCCL2 backend and the existing native allocation policy.

The integration supplies a current-profile C++ subclass of Torch's pluggable allocator, installed through its exported `changeCurrentAllocator(shared_ptr<CUDAAllocator>)`. Existing allocation, free, stream and graph-pool callbacks remain the implementation. The added operations describe actual native Working segments; an empty snapshot is returned only when there are actually no mapped segments.

Proposed native observation interface:

```rust
pub struct WorkingSegment {
    pub device: i32,
    pub address: u64,
    pub bytes: usize,
    pub size_class: usize,
    pub pool: Option<PoolId>,
}
pub trait WorkingObserver: Send + Sync {
    fn initialize(&self, segments: &[WorkingSegment]) -> Result<()>;
    fn mapped(&self, segment: WorkingSegment) -> Result<()>;
    fn retiring(&self, segment: WorkingSegment) -> Result<()>;
}
pub fn observe_working(observer: Arc<dyn WorkingObserver>) -> Result<()>;
```

The one process-local observer subscribes while the device registry and existing Working states are locked. The complete existing mapped inventory is delivered in one transactional initialize call before subscription becomes visible, under those same locks; future devices inherit the observer. Initial delivery failure does not publish a partial subscription. Native snapshots separately include actual per-segment block state and pool identity, without initiating CUDA or changing residency. All FFI calls check the existing process guard and contain Rust panics/C++ exceptions.

For future maps, notify only after successful mapping and before publishing an allocation or prepared pool to a consumer. For release, notify after the existing stream-completion and capture/pool ownership checks, before unmap/release. A failed notification poisons the owning Working state and retains its physical mapping; it does not unmap, reuse, or credit those bytes. Partially mapped or otherwise uncertain segments must refuse a snapshot rather than masquerade as successful mapped storage. No map callback occurs from a successful capture allocation: capture uses already prepared backing and continues to forbid mapping/recovery.

Callback lock order is native Working state, C++ listener registry, NCCL hook/communicator locks. Callbacks contain no Python, acquire no GIL and must not re-enter Varena. Snapshot construction never holds the C++ listener lock while calling native snapshot traversal. The native subscription handoff must not drop a map/free between its initial inventory and ongoing notifications.

Torch's separate `snapshot()` then `attachAllocatorTraceTracker()` calls are not an atomic public handshake. NCCL2 calls the latter from its singleton constructor; replaying events synchronously there would recursively initialize that same singleton. This upstream boundary must be qualified against actual executor group-before-model ordering, and must not be represented as a generally atomic Torch snapshot/listener API. The native subscription itself remains atomic.

Working storage and Region exports are different physical owners. The snapshot covers Working backing, including prepared graph pools, not weight pager/Cache mappings mislabeled as Torch allocations. Region exports already route allocator `recordStream` through `tensor_consumers::record_pointer`, retaining the actual Pin and consumer tails. Ordinary NCCL2 all-to-all passes tensor addresses directly and does not require the tensor to be in its registered-segment map; symmetric-window registration is a separate API and is outside the current Ulysses route. The GPU client must include an actual exported Region tensor as a collective input to verify this boundary, alongside ordinary and captured Working allocations.

Required real qualification: truthful nonempty snapshot; map/free ordering and callback-failure retention; actual two/four-rank NCCL2 construction, collective, checked completion and destruction; pre-existing allocation before group creation; late allocation/trim; foreign Region collective input; graph pool capture/replay then checked retirement. Preserve run26854. No GPU pass, old-backend fallback or release is claimed by this document.

Build profile: C++20, the actual Torch2.15.0+cu132 headers/libraries and matching CUDA runtime/CRT13.2.86. External SDK headers are system includes; project warnings stay errors. The subclass adds direct libtorch_cuda/c10_cuda/libcudart linkage. Historical 5fe metadata/ABI1 remain frozen; this same-cohort private function-table and installer metadata use ABI2. This does not rename or alter public vr_v1 APIs.
