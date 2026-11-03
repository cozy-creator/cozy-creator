# Pinned graph review handoff

Review source `4519f9e17037291dde0176801ab1500f2739db20`, draft #2479.
The older5a4/363/d5b source and receipts are historical, not the corrections below.
This handoff does not report a new CUDA run or relabel host evidence as GPU proof.

## Implemented replay guarantee

The implementation uses the **conservative selected-Source readiness barrier before
every epoch that acquires a native Use**. `_Epoch.acquire(capture=True/False)` first
acquires Use and ordinary typed exports, then unconditionally calls `used.wait_ready()`.
Both callers invoke acquire before actual capture or the host replay exclusion scope.
There is no non-waiting replay option in this frontend.

The wait checks selected same-generation native producer controls through completion
AND checked job/staging/resource retirement, surfacing their errors before this epoch's
graph submission. It releases the GIL and retains native custody. Its wait set is scoped;
the ABI retirement lease also temporarily excludes new device capture/replay admission.
It is not zero-cost or concurrency-neutral, and this frontend makes no fill/compute
overlap or performance claim. Completed/retired warm controls take the native fast path.

Producer ordering plus fresh checked exports alone would protect dependency/lifetime
safety and permit overlap, but would NOT acknowledge late selected producer/resource
errors before submission. That is a different error contract, not the current path.
Fences/quarantine remain required for custody in either case. No application retry
hides errors. Future graph/device faults and final persistent Source/service teardown
are separate error paths; wait_ready does not certify them or arbitrary Working,
static-input, RNG or library/workspace preparation.

## Exact source corrections

- `bd08f2eedc4a7d6be2a8ebf39851ae25b52afb15`: ended binding close releases sanctioned
  strong guard/table references only after complete capture/routing exit, before export
  release and C10 Storage probing. Active binding close refuses. Genuine detached
  Storage survives, is upgraded strongly and retains its actual native Pin custody.
  Capture uses epoch select/restore intent instead of an untracked local activation.
  Known intent deactivates before escape refusal; deactivation failure retains exact
  epoch/exports/Regions/Sources/Plan and marks the owner failed even without a parent.
- `f0989b046d29719e43ce8683e06437aacca8ab6f`: configuration includes supported scalar
  attributes and flat immutable tuples, including Conv2d stride/padding/dilation.
  Tensor/mutable tuple contents do not enter or gain ownership through the key.
- `7c60c40fce20ca785495215cb5d429570517373a`: helper snapshots actual caller C before
  switching, prepares the pool outside capture, orders S behind C when distinct and
  synchronizes S only. Native admission/routing/capture-end/footprint/error cleanup is
  preserved. Other preparation streams need explicit owner joins. Pool preparation
  may still wait/recover under pressure. No keyword/probe/global fallback remains.
- `7b3f0b3971b46e655c527738bedccb48a1f14d6f`: establishes the conservative every-Use-
  epoch readiness policy described above, after typed exports and before both guards.

Required native readiness implementation is549f49155 (or the equivalent already
composed186 implementation); root owns composition. Clients12425d44f and auditedb40f
proof are optional/separately scoped. No context4463, snapshot, new definition class,
StageSource, unpinned tensor or native interface is required. Ship frontend and helper
as one matched SDK cohort; frozen #2449 remains unchanged.

## Evidence and remaining gates

Guard proof8e4f845e8 records actual CPU C10 no-user-escape red/green and true detached
Storage retention with an ended binding still alive. Configuration proof4519f9e17
records actual Conv2d stride6x6->3x3, padding/dilation/nested key red/green controls,
exact inference/key restoration, and eleven CPU/native-host client passes. Commands,
source/artifact hashes and exit codes are in
`evidence/2026-11-02-graph-guard-retirement/manifest.json` and
`evidence/2026-11-03-graph-configuration/manifest.json`.

Those clients do NOT execute full CUDA GraphSegment construction, valid-Use GPU
readiness/late-error/GIL behavior, native active-Plan cleanup, physical reclamation,
CUDA concurrency or replay. They do not qualify coordinator771/873 H100 results.

Coordinator GPU gates remain open: normal capture with definitions alive/Pins0,
same-VA eviction/refill/parity; capture_escape with retained ordinary native Storage,
known Plan retired and model Plan reusable; uncertain capture/deactivation failure
retaining exact owners and explicit owner failure; separate parent replay-Plan phase
observation; delayed consumer/reset/Source error controls; and Conv tuple mutation
refusal before another actual replay with old output preservation. Prepared commands
and intentional child-lifetime retention boundaries are in GRAPH-COZY.md. No GPU,
machine, rental, merge or release action was performed for this handoff.
