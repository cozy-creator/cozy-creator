# Standalone native cooperation consumer

This release-gate helper links Varena directly, without Python or Torch. It allocates and
verifies real CUDA working memory, can own the native automatic service or act as a manual
idle donor, and reports actual allocator/driver facts. The metadata-only watcher observes an
exact donor row in `Waiting` for an exact requester; it neither reaps rows nor credits memory.

The ordinary Cozy fixture controller will kill only its own donor process after that observed
state and a matching PID/birth identity. A deadline without the marker is a failed check, never
authorization to kill or evidence of donor death. Reaping is not evidence of physical release.

The controller has three ordinary Cozy entrypoints: `smoke`, `donor_death` and
`simultaneous_auto`. It verifies the executed ELF hash, PID/start_ticks/pidfd and no
Python/Torch maps for every native child. Readback covers every requested byte using
bounded host windows. Watchers also prove that CUDA is not loaded.
Private ledgers are created explicitly under `/dev/shm` with mode 0700; default `/tmp`
is not assumed to satisfy the ledger's tmpfs requirement.

`donor_death` warms all contexts and small read/write paths, publishes real completed
idle working pages from a manual donor, and leaves that donor blocked on stdin without
service. A plain native filler creates physical pressure from observed driver free
minus an explicit margin. Only after `watch_ready` does the requester allocate through
its automatic native Owner; no controller command creates an ask. The exact Waiting
marker precedes owned donor termination. Initial physical exhaustion is preserved if
context release lags; any later allocation/readback is a separate result, gated by an
actual driver observation. A reaped row never counts as available bytes.

`simultaneous_auto` uses owned pipes to release two native work operations after both
are armed. Both own published idle cache and an automatic service. Actual allocator
intervals must overlap. Work reads back its complete output before freeing it; resulting
native recovery/donation facts distinguish real roles, successes and capacity failures.
At least one real allocation/full readback is required for `progress_qualified` and
the simultaneous cell's positive result. If both exhaust, overlapping bounded failure
can pass safety checks but the positive progress gate remains false. Donor-death's
`automatic_episode_progress` distinguishes the original request from a separate later
physical readback; a protocol/integrity pass does not erase its initial exhaustion.
Metadata snapshots may retry Busy; GPU allocations are never silently retried by the
controller. Cleanup targets only owned children and preserves receipts/directories if
termination does not finish.

Every native reply carries command identity, maps, working/live/mapped/driver counts
and automatic-owner facts. `prepare-controller.py` verifies ELF64/x86_64, GLIBC<=2.28,
no Python/Torch linkage, and exact source/archive/binary hashes before freezing the
project and an initially UNBOUND GPU input template.

Earlier fa731/fbb artifacts remain preserved as pre-withdrawal-fix evidence. This
fixture now consumes exact native `c3bf579ae82b42b254aa30fe54c609fc8e89afcb`, the
product owner's frozen withdrawal fix. It observes own demand/active recoveries/
pending withdrawals and unfinished owned rounds return to idle without issuing any
withdraw/clear_want/ask/service command. The four bounded GPU cells passed on
Hitoshiki; exact receipts and limits are in `evidence/2026-11-01-hitoshiki/`. Native
product code is imported unchanged from that checkpoint; this fixture introduces no
allocator, daemon or published tool behavior. Source/build success is not GPU proof.

The Python controller declares Torch because it reads the assigned GPU identity and
driver headroom through Runtime. This dependency applies only to the parent controller;
the separately executed Rust consumers still link and load neither Python nor Torch.
The first GPU attempt (25306) failed before fixture entry: the original controller
omitted Torch, which Runtime Start requires for device initialization. The worker
reported no acknowledgement of its negotiated memory policy. Preserve that startup failure separately from native proof.

`native_lifetime` is a separate optional entrypoint, independent of the three pressure
and progress cells. After real CUDA initialization and full canary readback it forks
the native process. The child requires exact `Inherited` refusals from Owner facts
and close, writes one fixed pipe byte, and calls `_exit`; it performs no CUDA calls,
heap formatting, stdio or Rust teardown. The parent owns the child PID/start ticks
and pidfd and preserves its exit result, then verifies the parent canary again.

The capture check opens a real manual instrumentation Peer before capture and retains
it until after all capture/close guards. It uses the exact two-argument CUDA
`cuStreamBeginCapture_v2` ABI. During real capture, Owner close must refuse and remain
stopping but not retired. For a defined 600ms interval, live/mapped working bytes and
the product working-driver counter must remain unchanged. Capture then ends, Owner
retirement completes, and full canary readback still passes. There are no Arena
weights: this measures Owner close deferral and product working-driver calls only,
not last-Arena teardown or absence of all external CUDA calls. An error ending CUDA
capture emits a failure receipt and exits the isolated helper without Rust teardown;
uncertain capture state never permits silent native retirement.
