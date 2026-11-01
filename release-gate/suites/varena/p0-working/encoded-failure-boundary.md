# Encoded scratch failure: proof boundary

Source inspected: TensorFS/Varena `cbfc677c7`, Runtime `a1d544a34` and the unchanged native execution provider.
This is a fixture feasibility note, not a product failure or GPU result.

The real `failed-source-fill` case covers a raw source error after native destination admission. It requires an
unpublished region, checked cleanup, preservation of the original error and a successful subsequent CUDA refill.
Raw IO has zero decode scratch. Existing native working/DLPack cases and real-model execution have their own
scratch/lifetime coverage; none is relabeled as an encoded source-error teardown test.

The public encoded path has a narrower testing seam:

1. `tensorfs.varena.LoadSlot.encoded(...)` validates the encoding and prepares a real unpublished destination.
   `prepare_encoded` in `crates/tensorfs-py/src/varena/refill.rs` allocates the packed device scratch through
   `io::DeviceBuffer::new` before payload IO. This gives a real observable scratch allocation.
2. Starting the read requires `LoadSlot.submit_native(continuation)`. The binding in `varena/load.rs` first
   imports a genuine `cozy.execution.continuation.v1` capability, then takes the sealed work and starts its worker.
   There is no public standalone `start()` for an encoded slot, unlike raw `Region.prepare_read`.
3. Runtime's current native execution contract says `author.denoise_loop` is its recorder/submission surface.
   The older TensorFS docstring mentioning `PreparedRepeat.capture(load=slot)` is stale: the current Runtime
   class has no such public capture method. Fabricating a capsule or constructing a private Runtime harness
   would not be a qualification of the existing public path.
4. `checkpoint.components[...].tensors.read(..., decode=True)` is synchronous. It exposes no callback or
   prepare/start boundary where this fixture can deterministically damage an owned source **after** scratch
   allocation. A racing file mutation would make the failure boundary uncertain.

Preparing and cancelling an encoded slot could exercise unused-scratch retirement, but it would not prove the
requested source-error path. Invalid encoding metadata can fail before scratch exists and is also insufficient.
No such substitute test was added. No driver/kernel mocking, capsule fabrication, product API changes or native
builds were performed.

The actionable P1 failure gate remains the real raw `failed-source-fill` case. If a later supported encoded
consumer exposes a deterministic admission/start boundary, its failure test should use that actual owner and
record scratch, unpublished destination, original error and final cleanup separately. The present absence of a
small public injection seam is not evidence of an allocator defect and does not expand the P1 acceptance matrix.
