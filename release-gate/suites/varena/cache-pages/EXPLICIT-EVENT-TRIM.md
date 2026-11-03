# Explicit eager event trim

This trim-only successor composes reviewed native771/Python873 with fetched
master cd9e20b95. Its feature delta is adapted from 2e6704f70; explicit-stream
deferral ca9 is not included.
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
core or a historical binary. The helper lock records its already-required direct
nvml-wrapper-sys and current platform dependencies. The unrelated owner-close
observer patch from the
original 2e commit is excluded; owner_retirement.rs remains byte-identical to873.

CPU/source checks do not qualify GPU behavior. If the original LM count still
changes, do not broaden this predicate to Live/pool metadata or raise a tolerance
merely to make that gate pass.

Qualification must pair the existing `event_trim`, P0 `gpu` cases
`retained-output`, `shared-pool`, and `replay-hook-failure`, and Runtime
`graph_owner_retirement`, `lm_owner`, and `lm_owner_warmed` against one exact
cohort. Preserve the fixed three warmup cycles and all original assertions.
The26122 stabilization failure happened before its main warmed baseline, and
the original LM+18/+1 event identities are not yet attributed. Source/host
checks do not establish GPU correctness or an unbounded leak.
