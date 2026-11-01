# Native foreign pressure: source audit and first implementation seam

This work starts from fresh Creator e28d759ce plus qualified native c3bf579ae;
the paired Runtime worktree starts from 640193706 plus4d5844f0. Those qualified
products and their GPU evidence remain frozen. Root's native/Runtime integrated
branches are the eventual targets; this audit does not rebase them or change the
separately owned Cache worktrees.

The target is one native memory policy. TensorD supplies lifecycle, sharing mode
and priorities, and ends idle workers. Runtime supplies actual lifecycle/pass
boundaries and safely retires owners when asked. Neither chooses GPU byte targets.

## What the current sources do

| Source | Current behavior | Consequence |
|---|---|---|
| `tensord/src/gpu_service.rs`, `Pressure`/`room` | Fixed H=max(512MiB,card/16), another call's band=max(512MiB,card/10); foreign baseline/growth latch, per-executor byte targets, idle offload/exit and running-set room. Steady foreign use holds H; continuing growth uses2H. | This is the shipping game-yield policy, not the v2 measured opaque-growth reserve. It must not be copied into a second native byte controller. |
| Same, `memory_modes.legacy(...)` guards | c3bf's Cooperative mode bypasses every legacy GPU byte-control entry. | Current Cooperative service has no foreign-pressure replacement. |
| `memory.rs`, `Registry::opened`/`usage_by` | GPU process identity can be inferred from context-opening PID cardinality and a learned context-size band; descendants are assigned to executors. | Useful shipping observations are not exact evidence for training local opaque H. A foreign process starting in an ambiguous window must remain unknown. |
| `gpu_service.rs::cooperation_settings` | Start carries actual UUID, ledger directory, machine Shared/Dedicated and priority0. | Sharing is real at initialization, but dynamic priority/lifecycle intent is not yet propagated. |
| Runtime `cooperation.Config`/`Memory.attach_cooperation` | Config binds once at fresh Memory creation. Old Room frames fail closed in Cooperative mode. | Preserve isolation; do not use a Room frame as a new native-policy channel. |
| Runtime `Memory.make_room/_room_floor/_take_room` | Legacy byte conversion, a floor involving peak/reserved plus streaming slots, and pass-boundary lift. | Keep the safe boundary, retire the Python byte formulas. The measured working peak remains a record, not a hard upper bound. |
| `cooperative::Peer::sample`/`ledger::Participant::ask` | Actual device observation is optimistic, H/attribution unknown; allocation asks exclude the requester. | Background pressure cannot simply reuse local-first allocation recovery: multiple processes could each spend the same device deficit before joining a round. |
| Qualified Plan4dc (`plan.rs`, `working/split.rs`) | Exact phase-region intent and generation/lease ownership; committed execution-key/prefetch remains open. | Regrowth must integrate with this native intent; it cannot claim the unfinished prefetch path is already complete. |

`cl-694` remains an open shipping finding: config supports `machine.mode`, reread
when the service starts, but the ordinary CLI lacks the mode setter. Its proposed
route is `cozy machine update --mode ...` followed by the existing restart. Keep
that ownership separate; coordinate before adding a minimal lifecycle setter in
our own branch. Fixture-only `Owner.configure(shared=...)` is not machine-mode proof.

## Native observation and attribution

Use separate typed facts:

- Actual driver free/total and observation generation/time. These are observations,
  not an allocation grant; CUDA documents races and no guarantee of allocating all
  reported free memory. [CUDA memory API](https://docs.nvidia.com/cuda/archive/13.4.2/cuda-driver-api/cuda_driver_api/group__CUDA__MEM.html)
- Exact native mapped holdings, with live, ready and pending subdivisions. Cache's
  new physical classes join the same accounting; a callback request is not a class
  of released bytes.
- Known foreign process growth only when the process list and identity attribution
  are usable. NVML compute/graphics process usage is dynamic; duplicate entries use
  one process's maximum, not a sum. Missing/unavailable values remain unknown.
  [NVML device queries](https://docs.nvidia.com/deploy/nvml-api/latest/api/group__nvmlDeviceQueries.html)
- Attributable local opaque growth, separately qualified. Never compute this from
  device-used minus managed bytes alone: that residual contains games, other CUDA
  contexts, modules and unaccounted activity. Never reuse context-size PID guessing
  as an attributable measurement.

H starts unknown. Only a qualified local observation window may update the largest
observed local opaque growth and its2×/32MiB-rounded reserve. Unknown is not zero.
Uncertain identity, overlap, unsupported accounting or an inconsistent managed/driver
observation must preserve unknown attribution, not clamp it into a positive proof.
Native mapping-call epochs can reject a reading that raced local physical changes;
they do not by themselves prove NVML attribution or accounting latency. Validate
native VMM-only growth/release and foreign-only growth as negative H-training arms.

The first native checkpoint will preserve unknown H rather than install a fixed
512MiB/card-fraction substitute. The reserve trigger stays inactive until measured
H exists. Where reliable process attribution is unavailable, the new-PID trigger
also stays unavailable; report that boundary explicitly. Shared idle release and
real demand-allocation recovery still work. A broader unattributed-growth trigger
would be a separate design change, not a hidden fallback.

## One device pressure round

Add a pressure entry point to the same ledger/worker accounting, distinct from a
physical allocation request. It may assign the sampling participant's own Shared
idle bytes. A foreign signal enters the coordinated round before physical release;
independent per-process local-first reclaim is forbidden for a shared deficit.
Concurrent observations join the existing round; only completed physical effects
count. Fresh observations, not previous promises, determine remaining pressure.

The first seam is narrow: include-self pressure assignments over already completed
native reclaimable pages, preserving c3bf's exact-generation ownership, withdrawal,
nonblocking publication and actual-driver accounting. Root agreed this boundary.
It gives a reviewable native checkpoint before attribution and owner callbacks are
composed. No general cache/foreign-pressure qualification follows from that alone.

Cache integration must keep **requestability** separate from **reclaimability**.
Retained graphs and live reconstructable caches may accept an owner request even
when their published reclaimable bytes are zero. The bridge is a nonwaiting,
coalesced request keyed by cause/observation/round. Finished/declined/failed callbacks
are distinct from a completed native physical-release epoch. Allocation callers
cannot wait on Python/GIL-dependent action. A background foreign round must advance
past an owner that declines without counting its requested bytes as capacity or
letting that owner hide a healthy donor. Coordinate the precise capability/answer
representation with the Cache owner; do not overload `physical.caches.live` as free.

Sampling stays on the native library's worker, not Runtime's Python pump or
TensorD's reclaim loop. Any polling/owner-action interval is a coordination choice,
never process-death evidence. Do not cancel an active request for foreign pressure.

## Retention and regrowth

Foreign pressure lowers native speculative retention/preload intent. Demand working
allocation and required Region admission still perform their legal reclaim/driver
path; a measured working peak cannot become a new hard cap on a later larger pass.
Existing explicit development caps remain distinct.

The existing outer-forward/request boundaries are mechanical notification sites.
Native policy samples and admits regrowth there, outside Memory/allocator/capture
locks. The sampling thread cannot authorize a mid-pass raise. Plan region intent
and the successful working sample constrain prefetch; unknown configurations retain
demand-only admission. Future actual allocation still decides success.

Shared/Dedicated must be read from the real machine configuration and verified in
Start, native row state and ordinary CLI receipts. Dedicated is not isolation from
unrelated programs. The final policy must state whether dedicated foreign-pressure
response follows v2's donation opt-out or shipping's emergency foreign-growth rule;
do not accidentally inherit whichever branch happens to run. Priorities/activity
are lifecycle categories, never byte targets.

## TensorD retirement boundary

Remove the CUDA `room_need/make_room/offload_idle/yield_to_call/relieve_pressure`,
per-executor room/lift targets and pressure admission parking after native gates
cover their supported consumers. Keep GPU-slot scheduling, warm-set/lifecycle
intent, host RAM pressure, disk management and exact idle-worker termination.
An idle process exit returns a context only when the driver actually returns it;
no row removal or termination request creates capacity credit.

If native pressure needs an idle context ended after safe memory has been returned,
report a typed lifecycle request/pressure state. TensorD may choose an actually idle
worker by lifecycle priority/LRU and recheck activity before ending it; it must not
reintroduce byte allocation, deficit subtraction or an active-call cancellation rung.

Linux native policy can progress in Cooperative mode while the guarded shipping
bridge remains for unsupported cohorts. Complete deletion requires a coherent
capability boundary and supported-platform checks. Windows needs its per-process
budget/event source; Linux device free memory is not a substitute. DXGI documents
application-specific usage/budget and only a hint, not a guaranteed reservation.
[DXGI budget fields](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info),
[residency notifications](https://learn.microsoft.com/en-us/windows/win32/direct3d12/residency).

## Real qualification sequence

1. Real ledger processes prove include-self assignment, simultaneous samplers,
   duplicate observations, exact withdrawal, stale/unknown readings and no credit
   for queued callbacks, pending events or a dead row. No GPU claim from metadata.
2. On an owned headless GPU, native-only device pressure returns actual idle working,
   Cache-ready and weight backing while retained live/pinned values survive.
3. Attribute local opaque-only growth separately from native VMM growth and a
   foreign stand-in; foreign growth must not train H. Namespace/unavailable readings
   remain unknown. Record the exact attribution source and uncertainty.
4. Use the actual machine Shared configuration route, Start categories/native row and
   priorities; return to Dedicated through the same supported lifecycle. Do not
   relabel direct fixture configuration as this proof.
5. Measure the workload's irreducible live/pinned floor and successful working demand.
   Run real SDXL/Qwen coexistence beside a stand-in game only when that floor fits.
   Separately exceed the floor and require clean exhaustion without package replay,
   active-call cancellation, corrupted values or fabricated reclaim credit.
6. Save pressure observation, owner action, completed physical release and actual
   driver free/foreign-allocation outcomes. Include delayed consumer events, graph
   owner retirement, game allocation failures and the launch-budget blind spot.
   Sampling cannot guarantee the first foreign allocation succeeds.
7. Oscillate pressure and observe reductions once per actual signal/round, with
   regrowth only at pass boundaries. Record mapped bytes, real transfer traffic and
   timing boundaries. A development cap is not physical small-card coexistence.

Root owns rentals, SDK changes and GPU execution. No paid action or new foreign-
pressure implementation has been performed by this audit checkpoint.
