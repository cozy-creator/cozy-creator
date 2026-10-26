# Varena: executor-local weight residency

This is an original implementation against the CUDA Driver API. Each executor owns its own
reservations and physical allocations. TensorFS supplies immutable bytes; it does not own device
memory. No source from aimdo or another external allocator was used.

```text
Arena::reserve -> Region (stable VA, no physical pages)
Region::prepare -> Resident(Pin) | Fill (exclusive initialization)
Fill::commit -> Pin (ready bytes)
foreign storage owns Pin until its final alias disappears
Arena::reclaim -> checked use completion (or context join) -> unmap -> release; Region keeps its VA
```

The Python/native bridge must retain a `Pin` in every exported storage deleter. A view or graph
can outlive Runtime's local references, so Runtime dropping an idle view is not permission to
unmap it. `Fill::pin` provides the same ownership for an in-flight fill tensor. Abandoned fills
never become ready; an escaped abandoned fill keeps its mapping pinned and blocks refilling.
The entire logical byte range must be initialized. `copy_from_host` checks the length; native
integrations using `commit_external` must uphold its documented unsafe initialization contract.
Consumers treat published weights as immutable.

Reservations and mappings use the device's minimum VMM allocation granularity. One Region is
one allocation and one eviction group: reserve at the execution block/group level, not per
scalar or page. Allocation has at most one finite reclaim pass and one retry after an actual
`cuMemCreate` capacity failure. It calls no Python, filesystem, TensorFS, network, or remote owner.

Runtime selects a preferred prefix with `set_preferred` and sets `set_streaming_limit` to its
largest block/window in CUDA-rounded bytes. Before a new allocation, the window counts only idle
nonpreferred pages touched in the current phase plus that new group. `reset_preferences` starts
a phase using the existing pin-recency clock. Ready cache last pinned before that boundary stays
as standby and is excluded from both the window's byte count and its eviction candidates. It
remains reclaimable by explicit/headroom requests and actual allocation pressure; it is not
promoted to preferred or pinned. A later pin makes it part of the current phase. Without a phase
boundary, or after clock saturation, the original global window applies. Cold/retired backing
is never exempted as useful standby. Already pinned common inputs and graph dependencies are
active working memory, excluded from the idle cache limit, and remain subject to actual CUDA
capacity. Dropping those pins can temporarily grow the idle cache; the next allocation trims it.
Preparing streamed groups evicts only idle nonpreferred groups. Activation
pressure via `reclaim(device, bytes)` can additionally evict the preferred suffix, permanently
lowering its watermark. New reservations at or after the watermark cannot become preferred while
any existing Region owner remains. After all old Regions and escaped pins are dropped, the next
reservation starts a fresh prefix; retired cached pages are ordinary reclaimable memory.
No scan can continually evict and rebuild the retained prefix. Pinned groups are
never candidates. Insufficient unpinned capacity returns a typed capacity failure.

`Stats.reclaimed_by_reason` separates successfully released bytes caused by the streaming
window, a failed weight allocation, the Torch allocation hook, and an explicit reclaim call.
These arena-lifetime counters partition `reclaimed_bytes`; compare snapshots to attribute an
interval. They count released backing, not disk reads, transfer traffic or GPU stalls. The
allocator category includes both local allocation ceilings and actual driver OOM, while an
explicit call can be deliberate offload. Ordinary Region destruction is not a cache eviction.

Ordinary foreign-storage release conservatively joins the CUDA primary context before reuse or
reclamation. This protects untracked side-stream work after a storage deleter has run.
A native integration may consume its own Pin through unsafe `into_completed_region` only after
checking every producer and consumer's completion and preventing later use through that Pin.
It cannot clear the fence history of a sibling or reclaim a surviving foreign alias.
Capture dependencies and graph roots retain their pins by default.
Hold process-wide `varena::enter_capture(device)` around capture to reject residency misses and
reclamation in every Arena on that device, even when capture starts before any Arena exists.
`Arena::enter_capture` wraps the same device guard and also retains that Arena.
Prepare before capture; retire graph roots before reclaiming their weights, then recapture.

The Python bridge releases native pin counts and aborted-fill state atomically before deferring
final reservation ownership to its existing cleanup worker. This does not add Python or worker
dependencies to allocation recovery. `Retired` is terminal: reclaim cannot overwrite it while
invalidating physical contents, and a new model can reset the preferred prefix once all old
reservation owners retire.

Keep the Arena alive for the executor lifetime, through every capture/replay using that device.
Final Arena teardown while an unrelated capture/replay guard is active can retain CUDA backing
and VA until process exit; deferred teardown can encounter a guard entered after Python released
its handle. This conservative limitation is not successful reclamation or a general temporary-Arena
teardown guarantee. No reaper or retry queue is provided in this candidate.

Any uncertain map, access, unmap, handle release, or device-completion failure poisons the Arena.
It stops serving addresses and deliberately retains affected VA/handles until process death.
`Error::OutOfMemory` is distinct from `Error::DeviceFault` and other CUDA failures. Recovery must
not treat device faults or poisoned state as capacity. A forked process cannot use an inherited
Arena. CUDA primary context reset while an Arena exists is unsupported.

`resident_bytes(device)` counts only this crate's physical allocations. It excludes the CUDA
context, PyTorch's allocator, and third-party direct allocations. A bridge may combine this value
with separately measured allocation reservations for a documented managed-memory ceiling; it
must not describe that as total GPU use.

Source checks are `cargo check -p varena` and `cargo clippy -p varena -- -D warnings`. Real CUDA
allocation pressure, escaped aliases, failed fills, side-stream completion, graph lifetime, and
device faults still require the production release gate. No mock/unit-test success substitutes
for that gate. This source change alone is not a GPU qualification or release.

CUDA references: [Driver virtual-memory API](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__VA.html),
[allocation properties](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/structCUmemAllocationProp__v1.html),
[access descriptor](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/structCUmemAccessDesc__v1.html).


## Checked native-release proof (2026-10-26)

Default-home CLI runs 18630–18632 on RTX 4070 Laptop / 8 GiB passed with TensorFS source
`d6e6cf209edc2c6741c36b490dc58dc1a8ac8b97` (wheel SHA256
`6929c5b983811250f3279ac2e04174498d1b2ca55ee4f649911734c939de76a5`), Runtime
`80793e9ce` (wheel `6836678e51bcfc02480236538c4c7410c909b84057a6db3373e04623f6764aa4`),
and Creator CLI `6862dee81` (`dbe800873e4e0fcfad1fe3f6182c664afb4e6054af19d2f96040a1f9aaad009a`).
This is a distinct mixed cohort, not all repositories at one source revision.

| Run | Case | Result |
|---|---|---|
| 18630 | Completed native refill | Reclaimed 16 MiB in 0.641 ms while the unrelated CUDA event remained incomplete. |
| 18631 | Ordinary storage release | Reclaimed 16 MiB after the conservative fence; 716.340 ms and unrelated event complete. |
| 18632 | Surviving storage alias | Reclaimed zero bytes, retained the 16 MiB pin, and left unrelated CUDA work running. |

Each cell then refilled the same VA, checked exact tensor contents, and finished with zero pinned
bytes and healthy ownership. The frozen fixture's calibrated sleep and allocations precede the
reclaim observation. These artificial controls establish fencing behavior; the times are not model
latency comparisons or a ComfyUI speedup. The buffered read used zero direct bytes, but the Writer
had warmed the file: cold page-cache population and transfer throughput remain unproven.
Sibling-release ordering, abandoned fills, multi-stage execution and full-model pressure still need
their own real gates. The exact prior CLI/SDK bytes were restored and observed ready/idle at
16:41:16 UTC. Raw receipts and source/fixture hashes:
`outputs/stock-torch-20261025/codex-resume-20261026/checked-region-cells/` and
`outputs/stock-torch-20261025/native-streaming-resume-20261026/completion-caller/GPU-REVIEW.json`.


The same distinct mixed cohort also passed 12 native-loading lifecycle criteria in
runs **18650–18661**, all in executor PID **1464799**. Seven requests completed and five
intentionally failed at their expected boundary:

- Post-capture host source failures in checked/event toy loading and the small SDXL helper
  failed only their request. Native and Runtime completion were proven, graph launches
  remained zero, and cleanup reported zero physical pins/model users with an unpoisoned arena.
- Stale source/layout mutations refused with zero launches and reads, preserving caller state.
- Healthy successor requests completed in the same executor after each failure; the small
  SDXL successor ran seven steps with exact eager parity and 1,639,688 declared/read bytes.
- A separate SDXL preflight case produced the expected pre-submit fallback signal with zero
  reads. It does not prove an eager recovery inference from the damaged source.

All three injected source failures report the O_DIRECT path; these are not a qualification
of failures on non-preferred buffered reads. There was no active request-cancellation or
empty-input case in this run set. Sibling-pin ordering, the new own-VA multi-stage candidate,
full-sized model pressure and comparative performance remain separate gates.
Actual prior CLI/SDK hashes were restored and observed ready/idle at 17:10:59 UTC.
Raw evidence: `outputs/stock-torch-20261025/codex-resume-20261026/checked-region-completion-cells/`;
independent review: `outputs/stock-torch-20261025/native-streaming-resume-20261026/completion-caller/GPU-COMPLETION-REGRESSION-REVIEW.json`.

### Existing-address source stages: real GPU proof

Ordinary CLI runs 18775 and 18777–18781 passed on an RTX 4070 Laptop GPU using
Creator `bdb5c0509325e1b93d5c678d5bf48021a679005f` and Runtime
`befc4431795103340bc84eeacf9940c58a5633ec`. Three real Backend/Placement regions
reserved 12 MiB at their model addresses while a 4 MiB managed weight window
forced native sequential refills. Checked and event-ordered submissions each
read exactly 4 MiB per stage, returned the exact expected output, retained
the original addresses, and finished with zero pinned bytes and 4 MiB resident.
Python bodies ran only twice during capture, never during native advancement.

An already-resident stage ran in each ordering mode with zero source bytes.
An escaped input view was refused by the actual physical-pin ownership check;
boundary metadata mutation was refused and restored before submission. These
six cells prove the one-pass mechanism, not repeated model inference,
cancellation/pressure behavior, release status, or a performance improvement.
Production adapters still require model identity and generation guards.

TensorFS wheel SHA256:
`4caecc57c619c872290c78d955c22e8571a5738f01d27443dae4bb252d81628a`.
Receipts: `outputs/stock-torch-20261025/codex-resume-20261026/own-va-stage-cells/RESULTS.json`;
independent review: `outputs/stock-torch-20261025/native-streaming-resume-20261026/own-va-stage/GPU-REVIEW.json`.

### Finite repeated source stages: real GPU proof

Ordinary CLI runs 18808–18811 passed on the same RTX 4070 Laptop GPU with
Creator `4d671f510b19818383f65e10f1c6d021107e9228` and Runtime
`e0b5b78e4e66d80fa029a375139a130c5aabb59c`. A native sequence advanced three
actual placed 4 MiB weight blocks and a sampler through 30 passes, between a
one-time prefix and suffix. Its managed weight window remained 4 MiB against
12 MiB of source weights. Each submission performed 122 graph launches and
read exactly 360 MiB; all three source observers accounted for all 30 passes.
The shared GPU timestep reached 30, output exactly matched an independent
eager streamed linear reference, and final physical pins were zero.

Both checked and event ordering passed changed-input second submissions using
the same definition, preserving the previously returned output. Python body
counts stayed at their two capture invocations. This proves repeated native
streaming and same-request definition reuse, not cross-request caching, a
complete diffusion model, pressure/cancellation coverage, or benchmark speed.

TensorFS wheel SHA256:
`79b58b39c70fbe5f3ffa4d7eebb2539349c228d2985204eb1aa691831ba19224`.
Receipts: `outputs/stock-torch-20261025/codex-resume-20261026/own-va-repeat-cells/RESULTS.json`;
independent review: `outputs/stock-torch-20261025/native-streaming-resume-20261026/own-va-repeat/GPU-REVIEW.json`.

### Warm model-stage capture: real GPU proof

Ordinary CLI runs 18914–18922 passed on the RTX 4070 Laptop GPU with Creator
`e7feb3f4543c76d175bd2bcd635f46d5ecb9c50b` and Runtime `9f81699db`.
The public model-stage adapter captured real placed raw blocks, including a
fully warm capture, and then streamed 30 passes through a 4 MiB managed window
against 12 MiB of source weights. Checked/event execution and changed-input
reuse matched the eager output with zero final pins. Changed fill, Region and
source identities refused before launch. Escaped aliases refused, and the next
valid request succeeded in the same executor (PID 1965675).

These nine gates qualify the warm/cold raw descriptor path. They do not qualify
the separate native INT8 producer/consumer or complete SDXL adapter. The first
SDXL gate separately reached its exact 30-step output check, then failed a
fixture assumption that an idle-cache limit was a total residency cap; that
entire gate remains unpassed pending its explicit bound/next-demand diagnostic.

TensorFS wheel SHA256:
`cf79ae0ae5dad4320a7520cfb2f6ec5a606889483ce07acf604436781e3ad40a`.
Receipts: `outputs/stock-torch-20261025/codex-resume-20261026/combined-model-stage-cells/native-model-stages-20261026/RESULTS.json`;
independent review: `outputs/stock-torch-20261025/native-streaming-resume-20261026/own-va-ready/GPU-REVIEW.json`.

### Adjacent native source lookahead: real GPU proof

Ordinary CLI runs 19006/19007 and 19021/19023/19025/19026 passed event/checked
execution, changed-input reuse, actual capacity refusal, cancellation after
speculation started, and a healthy same-executor successor. The cohort was
Creator `5a103bca8ca99290e435489447913bb61769df8c` and Runtime `466238c0a`.
The three actual 4 MiB placed blocks used native per-attempt links and the
existing no-reclaim refill worker. A completed speculative read contributed
4 MiB to the exact source ledger; a fully warm second submission correctly
read zero bytes rather than discarding useful cached weights to force I/O.

The tight-budget case refused speculation and completed through ordinary
demand loading. Cancellation after a speculative worker started drained all
accepted sources with zero final pins; the next request passed in the same
PID. The attempt restored its owned idle-window policy after checked whole
completion. These checks establish host ordering and lifecycle, not CUDA-event
copy/compute overlap, hidden transfer latency, or a model speedup. A previous
reuse assertion requiring nonzero warm reads (run 19008) was a fixture error
and is preserved as a failed run, not counted as qualification.

TensorFS wheel SHA256:
`865aecd116f9221fd1203dfb02dfd53e7ff8dd7fa661f8ea2203d9ae12f027c5`.
Receipts: `outputs/stock-torch-20261025/codex-resume-20261026/native-prefetch-cells/RESULTS.json`
and `native-prefetch-v2-resumed-cells/RESULTS.json`; independent review is in
the owning `xs103-native-source-lookahead/.task/GPU-REVIEW.json`.

### Native component-phase preference entry: real GPU proof

Ordinary CLI runs 19184–19187 passed on the RTX 4070 Laptop GPU with Creator
`a4a257c12a76fa62dcfdc60f1a865625c30562e5` and Runtime `3bb440712`.
Each successful four-component native SDXL submission made exactly three
preference transitions: grouped text encoders, the UNet repeated range, and
resident wide VAE. Thirty UNet passes did not reset the watermark per block or
iteration. Changed-input reuse created a fresh per-attempt phase controller.
Checked completion invalidated Python's prior phase cache, and the next ordinary
text-encoder phase recomputed its state.

Cancellation after native progress stopped before VAE entry (two transitions).
A private truncated source failed its request after checked cleanup (one
transition), and a healthy following request succeeded in the same PID 2369864.
These four criteria include one expected failed request. The policy protects
only incoming bytes actually Ready at entry, demotes earlier ready cache to
standby, and neither allocates, evicts nor changes the streaming window. Newly
filled missing weights retain the existing streaming/cache behavior; no dynamic
missing-prefix promotion or performance improvement is claimed.

TensorFS wheel SHA256:
`01758fae75767a216fdf5157a1f78ef445b210d61434e438ada16218a9eec8f6`.
Receipts: `outputs/stock-torch-20261025/codex-resume-20261026/native-phase-cells/RESULTS.json`;
independent review: `outputs/stock-torch-20261025/native-streaming-resume-20261026/native-phase-entry/GPU-REVIEW.json`.
