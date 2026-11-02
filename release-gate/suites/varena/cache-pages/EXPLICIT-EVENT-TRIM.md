# Explicit eager event trim

This successor is separate from frozen native6e22 and its GPU qualification.
Original event-history26102, cold LM26115, and fixed-three-cycle LM26122 remain
preserved. No cold/warmed LM assertion or warmup count changes here.

Public `working::trim` now reclaims confirmed-complete private event handles on
Ready eager blocks that are globally reusable. It runs after ordinary physical
trim under the existing capture/Working exclusion. It does not change normal
allocation, free, poll, internal recovery trim, Peer service, Live blocks, pending
stream reuse, graph-pool blocks, or Cache/pool event stores. Query precedes destroy;
only a successful destroy removes the entry. Driver errors retain the uncertain
handle under existing poisoned ownership. Event cleanup grants zero GPU bytes.

The `event_trim` ordinary controller entrypoint runs a real standalone native
child. One live512-byte anchor and one live1024-byte output keep their eager
segments alive. The child accumulates18 event handles across actual GPU fills
without explicit trim, proving ordinary allocation/poll/free retain event reuse.
It reuses that block as a Live allocation and proves trim leaves it untouched.

A separate1024-byte free waits behind an actual CPU-controlled stream gate and
copies into the still-owned output. An actual CUDA capture verifies trim performs
no native calls or metadata mutation. Outside capture, explicit trim must remove
only the18 completed events, retain the pending event/backing, return0 GPU bytes,
and add no native event waits, context joins or physical memory calls. Opening
the gate permits the pending event to retire. Full output/anchor readbacks and a
new allocation/write/read cycle prove the ownership boundary and reusable state.
Final physical/event counters must return exactly to baseline.

Build the helper against the exact new core/std cohort, never a second static
core or the frozen6e22 binary. The helper lock records its existing direct
nvml-wrapper-sys dependency. The previously qualified owner-close observer patch
is included unchanged: manual counters live in their separate table and the
actual service table uses registration-free Ledger observation.

CPU/source checks do not qualify GPU behavior. If the original LM count still
changes, do not broaden this predicate to Live/pool metadata or raise a tolerance
merely to make that gate pass.
