# Native contracts for public client integration

Coordinator product `569649a8cdcd4db8214624e4d08866e27700a954`, evidence
`3df797f56e70180d9ff7a45feec896c31b439b0a`, draft Creator #2397/#2426.
This composes the shared service, multi-arena Plans, owned asynchronous fills and
observer retirement. It includes the current-format hard cut. Never install the
older dfb baseline as a ledger downgrade. Host checks are distinct from GPU proof.

## Shared service

```rust
Owner::get_or_open(
    device: i32, descriptor: ledger::Device, directory: &Path,
    initial_sharing: ledger::Sharing, initial_priority: i32,
) -> cooperative::Result<Owner>
Owner::close(&self) -> cooperative::Result<()>
```

Each owner is one client lease. Acquisition validates the same device and cooperation
domain and inherits an existing service's configuration. Initial mode/priority apply
only when creating it. Nonfinal close releases only that client. Final close retains
a retryable old-Control ticket; an earlier close retry must never close a new service.
`facts()` exposes clients/closed separately from active/stopping/retired. Native code
owns unknown/overlapping request activity and H qualification. Bindings must adopt
this seam in both arrival orders; they must not open competing services or configure
an existing service as a constructor side effect. Public objects must retain their
client custody for their real lifetime, including children and pending cleanup.

## Multi-arena Plans

```rust
Plan::new(anchor: Arc<Arena>, phases: Vec<(String, Vec<Arc<Region>>)>)
    -> Result<Arc<Plan>>
Arena::id(&self) -> u64
Region::id(&self) -> u64
Region::arena_id(&self) -> u64
plan::inventory_generation(device: i32) -> Result<u64>
```

Regions may belong to several arenas on the same device. Arena/Region IDs are
process-global, checked and never reused; they are not compute-key components.
Plan retains real owners. Inventory epochs cover publication/logical retirement;
committed admission rechecks the epoch. Outside-phase policy is stable arena order
then per-arena policy, not a promised global LRU. Wrappers retain Sources/Regions but
add no admission, split, preload or victim policy.

## Owned asynchronous fill

```rust
unsafe FillOwners::new(streams: Vec<u64>, resources: Vec<Box<dyn FillResource>>)
    -> FillOwners
unsafe Fill::into_async(self, owners: FillOwners) -> FillSession
FillSession::begin(&mut self) -> Result<FillDestination>
unsafe FillSession::publish(&mut self) -> Result<FillTicket>
unsafe FillSession::abort(&mut self, cause: Error) -> Result<FillTicket>
FillTicket::facts(&self) -> Result<FillFacts>
unsafe Region::prepare_on(self: &Arc<Self>, stream: u64) -> Result<StreamPreparation>
unsafe Pin::order_on(&self, stream: u64) -> Result<()>
```

The native unsafe contracts require an explicit, fixed, retained producer set before
`begin`, no earlier unaccounted writes, and all host submission closed before seal.
`begin` prepares workers, exclusion and events before exposing the destination.
`StreamPreparation` is Ready(Pin), Ordered(Pin), or Fill(Fill). Ordered inserts actual
producer-event waits before reads on that consumer stream; extra consumers must also
order their streams and record their completion. Streams retain their actual identities.
Published is not Ready. Unrecorded events never prove completion; failed/unsealed fills
retain or quarantine the actual owners. Ticket drop is host-only metadata destruction.

`unsafe trait FillResource: Send + 'static` supplies
`retire(&mut self) -> Result<FillRetirement>` (Complete or Pending). This runs after
checked producer completion, under retirement exclusion and outside global locks.
It cannot wait for Python/GIL. Complete must leave the wrapper inert or transfer all
custody with equivalent safety. Pending and Err retain the exact unfinished resources.
Retirement failure and remaining resources are separate facts; resource retirement
never grants physical bytes. Fill and Arena jobs use one typed per-device queue;
producer-event polling remains independent. Actual TensorFS Source adoption is a
separate unfinished adapter; wrappers must not fabricate early Ready or infer producer
closure from one per-copy callback.

## Observer and Arena retirement

```rust
Arena::request_retirement(self: Arc<Self>)
    -> Result<ArenaRetirement, (Arc<Self>, Error)>
ArenaRetirement::facts(&self) -> Result<ArenaRetirementFacts>
ArenaRetirement::try_complete(&self) -> Result<bool>
```

Success transfers custody to a native job and duplicate requests coalesce. Failure
returns the input Arc. The native job detaches only persistent Peer/Cache observer
roots; real Region/Pin/Fill/Plan and temporary admission owners remain valid. Ticket
drop does not free GPU memory. Facts carry Pending/Complete/Unproven/Failed, detached
observer status and exact remaining physical bytes. Accepted finalizing metadata
remains counted while other arenas run. Missing consumer completion is explicit
Unproven quarantine, never a hidden context/device synchronization fallback.

P4 should replace its temporary last-Arena `Arc::try_unwrap` loop and duplicate GPU
finalizer with this handoff. Keep public handles/tickets reachable on retryable refusal;
do not force-drop references, infer physical release from Arc counts, or wait for GPU
completion under an ABI registry/GIL/global capture lock. Service lease retirement
and Arena retirement remain distinct lifetimes.

## Evidence boundary

Exact569 passes the integrated C/Rust/Python host proof in both load orders: five
identities agree and only one core mapping is loaded. C11/C++17 and standalone host
clients pass. Native/example and full TensorFS binding checks pass. GPU cases for
fills, observer retirement, multi-arena Plans and the combined Fill/consumer/Arena
sequence are authored but unrun. Portable/release bundles, public adapter adoption,
GIL/source lifetime checks and general Torch offload remain open. All PRs stay draft.
