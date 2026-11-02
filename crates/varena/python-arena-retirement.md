# Internal Python Arena root retirement

Owner: `/root/p3_owner_restart`, branch `codex/xs138-p4-pyarena-adoption`.
Base: fetched master551980 plus native coordinator
`c0f537f3f2aae99f61db0efb435d8ca57b8f6594`. External P4/ABI files are untouched.

The native checked interface remains:

```rust
Arena::request_retirement(self: Arc<Self>)
    -> Result<ArenaRetirement, (Arc<Self>, Error)>;
```

The new destructor adapter is:

```rust
Arena::defer_retirement(self: Arc<Self>)
    -> Result<ArenaRetirement, (Arc<Self>, Error)>;
```

An inherited PID refuses before any queue/metadata lock and returns the input Arc.
For the current process, a failed normal handoff retains that exact Arc and the
original failure in the existing native quarantine and returns a Failed host ticket.
No separate worker, queue, byte policy, GPU wait or Python attachment is introduced.
The P4 ABI finalizer can use the same interface when composed by its owner.

`PyArena` retains its frozen/thread-safe class contract with one root mutex and
Live/Transferring/Closed(ticket) states. `owner()` clones the actual native Arc before
native guards; operations admitted before close remain genuine owners. A checked
close releases the root mutex, detaches the GIL for native handoff, restores the Arc
on refusal, and returns an observable host ticket. Repeated close returns that same
native ticket. New operations on a closed/transferring root refuse explicitly.

Python destruction takes only its own root and calls native defer_retirement
directly. It does not send the Arena to TensorFS cleanup, which may be awaiting a
GIL flush. Inherited destruction preserves the existing no-lock/no-CUDA behavior.
Closed-ticket destruction owns host data only. Escaped Region/Pin/Fill/Plan/source
and transient operation references are never detached or revoked.

Mechanical owner-accessor callers are the native load-slot/encoded constructors,
Plan constructor and PhaseController constructor. The raw Source owner's refill
dispatch/module uses Region/Work/Source directly and needs no edit in this branch.

## Required proof boundary

The real GPU fixture must drop a Python Arena without first calling close/request,
while unrelated automatic service and Cache owners survive. Escaped Region/Pin and
tensor aliases must retain exact readbacks/physical bytes; their last recorded
consumer event then permits native release. A separate checked-close case retains
its ticket, rejects new root operations, and proves capture/real-owner pending state.
Source checks are not GPU qualification. Refused handoff must retain caller ownership
or native quarantine and the original reason; no CUDA mock or Arc-count polling is
used to authorize cleanup.

No current-format/compatibility changes, public P4 API changes, GPU action or release
claim belongs to this delta. Source and staged-fixture checks are pending.
