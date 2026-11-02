# Owned asynchronous Fill proposal

Owner: /root/retention_restart. Base: coordinator106581cb66f161a218672032ebcbc0b59b89dd01
on freshly fetched master1b77958bdfb3f699db8340951c9b5cc1805d4243.
Root approved the ownership/state machine with demand-only prepare_on, metadata Session
facts and worker startup before exposure. Native implementation is in progress, unqualified.
The external P4 worktree is unchanged. Existing synchronous APIs stay in place.

## Current ownership

- `src/lib.rs:209-247`: Contents is Cold/Loading/Ready/Retired. Uses owns pins,
  unjoined status and last-consumer fences. A ready Pin is not a producer completion event.
- `src/lib.rs:1616-1760`: prepare returns Busy during Loading; only Ready is an ordinary
  resident hit. Failed-fill aliases keep their independent Pins.
- `src/lib.rs:1795-1930`: Fill owns a Pin. `commit_external` joins the context before
  Ready; `commit_completed` requires a caller-proven join. Drop marks Cold and keeps
  conservative unjoined reclamation. These contracts must not silently change.
- `src/abi.rs:376-443`: the foreign Source callback is synchronous with respect to host
  submission, followed by `commit_external`. P4 deliberately retains that checked join.
- `tensorfs-py/src/varena/refill.rs:195-210,399-449,698-748,803-898,1424-1468`:
  Work already owns source/checkpoint, input Fill/Pin, decoder storage, host buffer and
  ProducerGuard. It can order a closed continuation after issued producer work, but ordinary
  Region publication waits for source/decoder/continuation checked completion.
- `tensorfs/src/io/mod.rs:306-338`: successful read proves completion; failed reads require
  checked drain and separately checked source streams before owners may be released.
  `io::Issued` reports submission, never that completion. The current TensorFS read API
  remains synchronous; this proposal does not relabel its return or callback.

## Proposed native Rust surface

Keep this as a narrow ownership/event API, with no allocator callback registry or byte policy.
Owned resources have a checked native retirement contract, not a completion callback.
This successor replaces4f's provisional Drop-only envelope; no compatibility overload remains.
Producer completion and retirement failure are separate facts, and failed retirement keeps
the remaining objects plus ProducerGuard in quarantine.

```rust
pub enum FillRetirement { Complete, Pending }
pub unsafe trait FillResource: Send + 'static {
    fn retire(&mut self) -> Result<FillRetirement>;
}
pub struct FillOwners { /* fixed streams plus checked source/staging/stream resources */ }
impl FillOwners {
    // Host-only; same-device primary-context streams. Owners must keep every stream,
    // direct host/device input, staging allocation and producer library alive.
    // Resource destructors must be native or merely enqueue a frontend finalizer;
    // they must not wait for the GIL. No extra host submission may outlive
    // publish/abort's declared closure.
    pub unsafe fn new(
        producer_streams: Vec<u64>,
        owners: Vec<Box<dyn FillResource>>,
    ) -> Self;
}

pub struct FillSession; // unique producer-side owner; no Clone
pub struct FillTicket;  // host-only observation handle; no destructor CUDA calls

impl Fill {
    // Infallible host-only custody transfer, before any possible asynchronous write.
    // Incoming Fill must have no previous unaccounted producer; escaped views still own Pins.
    pub unsafe fn into_async(self, owners: FillOwners) -> FillSession;
}
impl FillSession {
    // Retains everything on refusal. Acquire ProducerGuard, validate all streams,
    // precreate all completion events and ensure the completion worker exists, then
    // expose a destination for submissions. OS worker-start failure retains custody.
    pub fn facts(&self) -> Result<FillFacts>; // host-only, also after partial failure
    pub fn begin(&mut self) -> Result<FillDestination>;

    // Seal all future producer submissions, record one completion event per actual
    // producer, then publish availability for explicitly ordered consumers.
    // Safety: every logical byte was initialized or its write was successfully enqueued;
    // no host thread may submit another write through the destination afterward.
    pub unsafe fn publish(&mut self) -> Result<FillTicket>;

    // Same submission-closure obligation, without a whole-content claim. Record/drain
    // the actual submitted work, retain failure in facts, and never publish contents.
    pub unsafe fn abort(&mut self, cause: Error) -> Result<FillTicket>;
}

pub struct FillDestination { pub device: i32, pub ptr: u64, pub bytes: usize }
pub enum FillState { Prepared, Writing, Submitted, Published, Completed, Quarantined }
pub struct FillFacts {
    pub state: FillState,
    pub submission_closed: bool,
    pub published: bool,
    pub completed: bool,
    pub retirement_finished: bool, // native ownership retired; never capacity credit
    pub producer_count: usize,
    pub recorded_count: usize,
    pub completed_count: usize,
    pub failure: Option<Error>,
    pub retirement_failure: Option<Error>,
    pub resources_remaining: usize,
}
impl FillTicket {
    pub fn facts(&self) -> Result<FillFacts>; // metadata only, never a query/wait in an allocator
}

pub enum StreamPreparation { Ready(Pin), Ordered(Pin), Fill(Fill) }
impl Region {
    // Existing prepare variants remain completed-only. This explicit new route also
    // accepts a published/in-flight fill after ordering the declared consumer stream.
    pub unsafe fn prepare_on(
        self: &Arc<Self>, consumer_stream: u64,
    ) -> Result<StreamPreparation>;
}
impl Pin {
    // Required BEFORE enqueueing reads on each additional consumer stream. Raw stream
    // handle must be live in this primary context. Does not establish consumer completion.
    pub unsafe fn order_on(&self, consumer_stream: u64) -> Result<()>;
}
```

`FillOwners` is fixed before `begin`: no late producer appears after a subset was fenced.
Resource values may own interior preparation state, but all actual writing streams must
already be listed. Initial support is one GPU/primary context and explicit stream handles;
no guessed/default producer set and no foreign asynchronous host writer continuing after
publish/abort. Source adapters that cannot close that inventory keep the synchronous route.
The empty-stream case refuses; synchronous host-copy fills keep `copy_from_host/commit`.

The unsafe contracts concern external stream/pointer/resource truth, as the existing
external Fill APIs do. The core records and queries its own events; the adapter cannot
supply a `completed=true` flag or an arbitrary completion callback.

## State and ownership transitions

1. Prepared owns Fill and all resources before CUDA submission. `begin(&mut self)` checks
   PID/capture first and creates events under producer exclusion. Capture/validation refusal
   keeps the same reachable Session and all resources; no by-value error loses its owner. Worker startup is also checked before Writing,
   so OS thread-creation failure cannot strand a newly submitted producer.
2. Writing begins before the destination is handed to source code. The Session owns a real
   ProducerGuard. Its existing contract remains: capture is excluded until actual producer
   completion, while unrelated resident graph replay remains allowed.
3. publish/abort declares host submission closed, then records *every* producer event.
   Submitted is not completed. Only successful recording of the entire set allows Published
   (valid contents) or a failed/aborted pending state. A partial event-record failure cannot
   yield a reader Pin, Ready state or reclaimed-byte credit.
4. Published is a new internal Contents state distinct from Ready. Existing `prepare`,
   `try_pin`, `resident`, raw prefetch and old C Source calls do not see it as completed.
   `prepare_on` obtains a physical Pin and installs waits for every producer event before
   returning Ordered. Each new stream must go through this operation or Pin.order_on.
   Consumer release remains the existing checked/fenced/unjoined Pin contract. Releasing
   a consumer Pin does not release the producer's own Pin. The ordered-Pin path must
   also preserve last-consumer fences while Contents is Published: the current
   `record_fences` Ready-only test needs a generation-checked extension, not a silent
   fallback to treating producer completion as consumer completion.
5. A per-device native completion worker owns the submitted Fill, ProducerGuard, event set and all
   source/staging/stream resources. It queries real events; successful completion of the
   entire set changes Published to Ready, or an aborted fill to Cold. Only then can source
   resources and the producer Pin retire. An ordinary consumer may still own independent
   Pins and fences. Published does not mean CPU-visible complete, and callback return is
   never completion/reclaim credit.
6. Dropping a submitted ticket merely drops its observer. The native worker retains the
   owners until completion/retirement. Dropping a Session before begin transfers its still-
   unsubmitted custody to deferred cleanup. Dropping Writing without an explicit closed-
   submission publish/abort is unproven abandonment: quarantine actual owners and backing,
   rather than guessing that no host producer will enqueue again. Normal adapter failures
   must call abort once they can establish submission closure.
7. A device error or failed completion query poisons/quarantines
   the actual fill and retains its owners. It must not manufacture Cold/Ready completion.
   Checked abort completion permits `abort_completed` and later `release_unready`; an escaped
   failed-fill Pin still prevents reuse. Physical release defers during capture/replay.

Worker resources must not be stored directly in a public metadata object's destructor.
No CUDA wait/destructor occurs while holding capture/arena/working/ledger/ABI-registry locks.
Every potentially final Region/Arena/resource owner leaves those guards before destruction;
final ownership release uses retirement exclusion, or stays queued for a later legal attempt.
Metadata observations cannot accidentally become the final GPU-owning Arc under a mutex.
The shared typed per-device retirement queue performs final Arena and Fill resource
retirement; final resource destruction is not performed on the producer-event polling
thread. Native Arena retirement49685f087 removes the old context-wide join and global
capture-mutex coupling. The composed source still requires real GPU qualification,
including separate final owners and cross-device progress.
`retirement_finished` is set after native owners and exclusion tokens are released; it is
not a statement that an escaped alias or frontend finalizer released physical memory.
Both workers start before destination exposure. These are genuine owner leases, not the
Peer/CacheOwner observer Arena references covered by the separate native detach seam.
The worker must progress without a Python callback or GIL; it only queries owned CUDA events
and retires native owners. Completed reports GPU producer completion, not a promise that
all external resource destructors succeeded. A Python Source's retained references need a separate GIL-safe
finalizer adapter after the core has proven completion, outside allocator/ledger locks.

## Ordering details

Completion events are unique for this fill generation and never re-recorded while an
ordered consumer can refer to them. Core ordering and worker event destruction serialize
on ticket state; either waits are successfully enqueued while those events are retained,
or all producers are already checked complete. Neither path waits on the GPU in prepare_on.
The returned Pin retains the real Region; subsequent consumer lifetime is independent.

CUDA's stream wait orders subsequent work after the recorded event; event recording is
not a completion assertion. An event created but never recorded must never be accepted as
proof. The successor follows the current official CUDA13.4 stream/event contracts, rechecked
on2026-11-01:
https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__STREAM.html
https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__EVENT.html
The immutable4f proposal references CUDA13.0; its source/build receipts remain scoped to
that checkpoint. These documentation checks do not qualify execution on a CUDA13.4 driver.

A partial consumer wait failure returns no usable Pin and retains all producer ownership.
Previously enqueued waits are harmless ordering, not writes or accepted operation progress.
Actual capture rejects an unfinished fill before any wait/event/device call. Completed Ready
pins retain existing capture behavior; no new asynchronous fill is admitted during capture.

## Adoption and proof order

1. Native core only: owned streams/staging, two actual producer streams and two separately
   ordered consumers, early published observation versus later completion, exact contents.
2. Real failure/ownership cells: partial enqueue then declared abort, drop submitted ticket,
   drop unsealed Writing, escaped failed-fill aliases, partial event failure with actual CUDA
   error if safely injectable without product probes, capture refusal/retry, capture-close
   deferral, inherited PID refusal, concurrent Region close/last Arena ownership.
3. TensorFS adapter: lift an owned read submission from its current engine, preserving the
   existing Source/Work owners and failure drain. An io::Issued callback alone is insufficient:
   all copy indices, decode producers, zero padding and direct source streams must be covered.
   Keep ordinary `read`, `PreparedRead`, commit_completed and their tests unchanged initially.
4. P4 adapter: add an explicitly asynchronous Source capability only after this native seam
   and real tests are reviewed. Existing fill callback, checked context join and private
   library/version contract stay intact. No Python/ABI local resident-state or byte policy.

Approved scope: fixed producer inventory and native owned-resource envelope, Published
separate from Ready, explicit prepare_on/Pin.order_on, and unsealed-Writing quarantine.
prepare_on uses ordinary demand admission; no public Admission/Prefetch mode is added.
The public C spelling/version remains P4-owned after native implementation and qualification.
