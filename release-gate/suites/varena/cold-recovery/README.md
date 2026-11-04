# Cold Working allocation recovery

Owner: `/root/vae_capacity_native_nov4`. Product source: `6c598b9f425984c4525e0b2cf2bd8f5cb61ae2dd`.
This suite preserves actual run 27462 as the baseline: first-call SDXL VAE
up-block 2 requested 1 GiB after the UNet graph caches had accumulated, under a
6 GiB managed cap. The caller failed before asynchronous owner service supplied
enough capacity. Zero driver physical OOMs were recorded.

The private cold entry opts in one native eager phase token and its calling
thread. It performs no reclamation. Only a real non-pool Working refusal, after
ordinary finite local reclamation, may observe the already-queued Allocation
request until the existing recovery deadline. Actual physical release permits a
real allocation retry. A callback return is not capacity. Timeout drops only the
observer: the coalesced request and its callback/arena custody stay with the worker.

Eligibility is latched on the first actual local refusal, including exclusions,
and survives the allocator's own subsequent callback activity. Explicit external
callback scopes, other threads, ordinary phases, Region recovery, and private
capture-pool allocation do not receive this permission. Every accepted cold
recovery join invalidates the exact observation before waiting. A successful body
can return its real result, but this observation cannot publish a clean sample;
a later natural invocation may observe one. No body is replayed.

`Job.changed` is paired with `Job.state`, whose guarded predicates include
finished/cancelled state. Normal action completion, ready collection, cancellation
and terminal transitions notify the job. Allocation observers may safely consume
the remaining deadline on a worker-wide error; they never import Admission's
callback-drain requirement. Platform Signal APIs and ordinary recovery semantics
are unchanged.

## Host control

`host.py` archives the exact product source into its output directory, appends the
visibility-local `host.rs` release client, and builds a separate dylib/executable.
It runs real native worker callbacks with real channels, requests, deadline waits,
coalescing and ownership. It fabricates no physical release, CUDA participant or
Working owner. The marker control verifies the actual external-callback counter's
scope lifetime; allocation behavior inside that marker is a separate GPU control.

Run with task-private Cargo and target paths, `nice -n 19`, one CPU and one Cargo
job. Corrected host pass `outputs/xs138-27462-vae-native-20261104/implementation/host-004/`
records the exact source hashes, 150 ms non-draining timeout, and terminal completion before its independent three-second deadline. Earlier host-001
(standalone manifest inheritance) and host-002 (dylib/std linkage flags) failures
remain preserved. Neither was a product-source failure.

## Actual CUDA controls

The coordinator runs `gpu.py` through an ordinary owned Cozy child, one fresh
child per case. The SDK wheel, native core and helper must come from the same
sealed SDK closure. The worker carries its own native closure. Never replace one
core with the other. Compile the unchanged
`../cache-pages/controller/ci_varena_cache_pages/gil_resize.c` as `gil_resize.so`
before sealing assets; pass that path with `--gil-helper` for `timeout`.

| Case | Actual assertion |
|---|---|
| `progress` | First real 16 MiB release is insufficient for a 32 MiB request; a second callback remains active while allocation is pending after the first release, then releases the other 16 MiB. The allocation returns only after enough release, the live sentinel remains unchanged, the recovered sample stays unpublished, and a later clean invocation publishes. |
| `timeout` | The existing C/PyDLL helper proves Storage.resize_ holds the GIL before and after real allocation refusal. Native waiting ends at the existing deadline with the original OOM and no entered Python callback; the queued owner later progresses. |
| `external-callback` | An allocation begun inside the explicit callback marker remains nonwaiting while an owner is unavailable. |
| `ordinary` | An ordinary eager phase does not gain the new wait. |
| `other-thread` | Another thread cannot inherit the opted-in phase's permission. |
| `no-pressure` | Cold entry with actual capacity preserves both existing caches and invokes no callback. |

The progress case is real multi-owner pressure evidence, not a deterministic
red/green proof of the prior eligibility race: scheduling can let an old caller
recheck eligibility before the second callback enters. No exact failure-edge
tracer or old-cohort run is claimed by this client.

The small caps are fixture geometry for actual allocator/owner contracts. They
are not substitutes for the unchanged full SDXL oracle: 1024x1024, 20 steps,
6 GiB managed cap, both requested calls once, and actual output checks. Run that
oracle with the original settings and workspace-cache environment unset/native
getter 0. Keep captured/replay/Region refusal regression clients in the gate.
No GPU case or full-model success is claimed by host evidence.
