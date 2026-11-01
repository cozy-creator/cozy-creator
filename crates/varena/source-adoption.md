# Source adoption and pre-launch hard cut

Owner /root/retention_restart. Follow-up branch xs138-p4-source-adoption starts from fetched
masterea521be3c plus frozen async primitive4f3f6c9b725b55aa85aed762aa68befeae96571c.
Tracker40cd9957433f39cf32f69112809f92d34ebb0929 is the decision source: old Varena versions
are not a compatibility target. Preserve ownership checks, migrate active users, then delete
superseded interfaces/joins. Historical frozen cohorts remain evidence, not a support promise.
The public C/Python/Rust surface spelling is P4-owned; this lane owns native Source adoption.

## Actual active joins

| Producer/caller | Current completion | Required migration |
|---|---|---|
| `varena/src/abi.rs:401-439`, host and foreign Source | Host copy then commit, or foreign callback then `Fill::commit_external` context join | P4 supplies explicit consumer stream and native owned producer/resource inventory; no adapter-local Ready state |
| `tensorfs-py/src/varena/mod.rs:363-390`, `PyUse.commit` | Raw destination ownership plus context-wide external commit | Runtime's real producers must declare all streams/resources before writes; current DLPack exporter ignores consumer stream because data is joined |
| Runtime `residency.py:_final_from_host` | Cached host page copied to a final Region, then `use.commit()` | Actual host page owner and the actual submitted Torch stream; bounded event completion |
| Runtime `residency.py:_ensure` | Source reads, optional host-page DMA and `model.fill` transforms, then commit | Native raw Source first; transformed fills need an explicit closed producer contract, including side streams |
| Runtime `residency.py:_assign` | Final decode/cast writes followed by commit | Declare normal current-stream writes and any actual decode streams; keep raw/final owners through completion |
| `tensorfs-py/src/varena/refill.rs:1448-1468`, `publish_inputs` | Typed Work already joins source, decode and possibly continuation before `commit_completed` | Adopt the owned raw read/Fill ticket, then encoded and continuation forms with their exact producer closures |

This inventory used Runtime534023328's actual call sites and native4f's code. Public stream
defaults in `varena/api.py` and the duplicate public surfaces belong to the P4 hard cut;
this branch does not independently edit the external P4 worktree.

Framework CUDA stream0 means an actual submitted queue with CUDA default-stream semantics.
It is not a synonym for an old Varena version. New low-level functions require the stream
argument; they must not choose0 when omitted or reinterpret an explicit framework0 as a
new private stream. Varena/TensorFS-owned producer streams use STREAM_NON_BLOCKING.

## First vertical slice: raw TensorFS device Source

The existing `Source::read_controlled` validates item ranges, fills only uncovered padding,
retains verified source files and aggregates every coalesced run into each placement's
issue count. `Engine::run` owns fixed native non-blocking copy streams and a bounded staging
ring. Its scoped CPU readers join at `io/device.rs:759-764`; then it records/waits end events
at770-790 and releases RunHold at802. This is the actual submission/completion seam.

Move the successful raw device submission into an owned native read object at that boundary.
Do not simply rename an `io::Issued` callback or return early while its CPU readers can still
submit. All placements, zero padding and all copy streams must have closed submission.
Source/decode/continuation forms with additional streams stay explicit separate inventory.

The initial adopter is the raw `PreparedRead` Work with no host-page export, no decoder and
no continuation. Work already owns the checkpoint/Source/items/Fill and a ProducerGuard.
It can transfer its missing inputs into native Sessions with the fixed read-engine streams,
submit all actual bytes, then publish each valid Region with event-ordered consumers.
Cancellation/partial reads seal an abort only after CPU writers have stopped; errors that do
not establish closure quarantine all actual owners. No package operation is retried.

The read engine's ordinary checked read remains during migration; it is not a permanent
old-version fallback. A successful owned submission may return read-byte/submission stats,
but completion timing/busy_ns and ordinary Ready publication require real recorded events.
Keep submitted and completed stats named separately. Existing CPU/file output cannot be
reclassified as asynchronous GPU completion.

## Concrete checked-retirement requirement

`io/device.rs::Engine::checked_release` can fail while releasing HostPin or DeviceBuffer.
`Engine::release` keeps the actual Retained owners on that failure. The current4f envelope
is `Vec<Box<dyn Send>>`; Drop cannot return this checked result. Calling the old release in
Drop and declaring full cleanup would hide an existing native failure boundary.

Root approved the narrow native contract; the successor replaces the provisional envelope:

```rust
// This is native fill-resource lifetime only, not a callback registry or allocator policy.
pub unsafe trait FillResource: Send + 'static {
    // Called after every producer is checked complete, under retirement exclusion and
    // outside capture/arena/working/ledger/ABI registry locks. No GIL wait.
    // Pending retains actual unfinished ownership; Err preserves every still-owned
    // resource and forbids further submission/retry. Complete leaves the wrapper inert
    // or proves an equivalent ownership/exclusion transfer. None grants physical bytes.
    fn retire(&mut self) -> Result<FillRetirement>; // Complete | Pending
}
// FillOwners owns the actual boxed objects until retire succeeds. A failed resource and
// every unprocessed resource remain with the producer guard in the quarantined payload.
```

A simple native owned value can use an internal wrapper whose retire drops it only after
completion. TensorFS implements checked release for its real owned read state. No public
completion boolean or arbitrary completion predicate is accepted. Root chose the checked
contract; no dual Box<dyn Send> compatibility API is retained. FillFacts exposes producer failure, retirement_failure, resources_remaining
and retirement_finished separately. Permanent failures quarantine the exact remaining owners.

The shared native retirement queue is being implemented by the observer-retirement owner:
`retirement::enqueue(device, Job) -> Result<(), (Error, Job)>`. After that source freezes,
add the typed FillResources job to that queue and remove the separate private fill-retire
queue. The independent producer-event polling queue remains. Arena/Region/Pin/source leases
here are genuine owners; observer detachment must not remove them or infer Arc counts.

## Later active users and deletion

- Encoded native input adds its actual decoder stream, packed DeviceBuffer, native decoder
  object and any final conversion resources before publication. A decode issue callback
  alone is not a source completion proof.
- Native continuation already has real start_after/checked completion; keep its real input,
  execution and output lifetimes, including escaped views. Change publication timing only
  when consumer ordering and failed execution still retain the complete owner set.
- Framework/Runtime writes must use their actual submitted streams. Known internal hooks
  can receive the new explicit contract; arbitrary custom hooks cannot be guessed to use
  only the caller's current stream. Refuse undeclared producers before writes, or keep that
  active path joined until its producer declaration migrates.
- Remove context-joining external commit and old implicit-default/public aliases once those
  actual callers are migrated and qualified. Incompatible current state fails safely; no
  old-layout translation, rollback to old byte policy or fabricated ready data is introduced.

Source4f's four native cases are not qualification of these adapters. The next real test
must read an actual TensorFS checkpoint into Regions, observe native in-flight publication,
order actual consumer streams, verify complete bytes and source counters, and exercise
cancellation/error/escaped aliases plus exact final accounting. GPU work remains root-owned.

## Current checkpoint boundaries

Frozen4f's native/example and complete TensorFS binding cargo checks passed in the root
build lane (native-check-002 and bindings-check-001; bindings2m55s). No GPU result follows
from those checks. The checked FillResource successor requires its own compilation and
real retirement Pending/failure plus raw Source consumer proof.

Current NVIDIA13.4 documentation confirms that non-blocking streams do not implicitly
synchronize with stream0, and unrecorded events contain no submitted work. The implementation
therefore keeps explicit stream identities and requires all producer event records before
publication; a successful query of an unrecorded event is never a completion proof.
References: [stream management](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__STREAM.html),
[event management](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__EVENT.html).
Historical CUDA13.0 receipts are preserved; no driver/toolkit upgrade or13.4 execution is claimed.
