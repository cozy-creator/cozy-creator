# Automatic Runtime donor owner

This ordinary Cozy GPU application requires the paired TensorD / TensorFS / Runtime
owner implementation. Startup must have negotiated Cooperative mode before CUDA
Start; the handler verifies its injected Context and the executor's actual
`Request.memory.cooperation`. It never creates a replacement Memory, Request or
Runtime donor owner, and never calls the donor's `publish` or `service` methods.

The existing Runtime int8 readiness probe is joined before taking the live-memory
baseline. A fresh Python child joins the same verified UUID/tmpfs domain. The
handler retains 8 MiB of live working memory and 8 MiB of pinned weights, plus
32 MiB of completed free working storage and 16 MiB of idle weights. The child
asks for 48 MiB; the Runtime's own pump must observe and answer the ask while the
handler is waiting and no new TensorD command arrives. The child then allocates
and checks 32 MiB, records real driver observations and retires its allocations.
The retained parent values must stay correct. Duplicate asks cannot produce
another credit.

Startup may be Dedicated. The test records that exact category, temporarily opts
its own participant into Shared through the existing native categorical API and
restores the startup sharing/priority in `finally`. This is explicit test-owner
intent, not a byte target. Consequently this does **not** qualify TensorD's
shared-machine configuration propagation or game coexistence. Root must also
inspect the ordinary run/worker notes for `gpu_memory_policy=Cooperative` and the
absence of legacy Room/automatic Map(Ram) byte fallback in this cohort.

The fixture waits for an actual automatic deferral under CUDA graph capture and
the native replay exclusion guard. It requires zero native driver-call delta in
both intervals. Ordinary frame memory may remain owned by the executor's two
library workspaces; fixture-owned allocations return to the exact settled live
baseline, with no pending bytes or extra graph pools. A second `observe` request
must run in the same PID with the same participant incarnation.

Run only through ordinary Cozy with exact artifacts, outside frozen package
source:

```bash
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl \
  GPU-OWNED-UUID /absolute/results/input.json
cozy run /absolute/frozen/owner-runtime/gpu --on OWNED --gpus=1 \
  --input /absolute/results/input.json --json
```

Require the actual result's `passed` and detailed assertions, not merely a CLI
completion status. Save the full run receipt. Add `expected_pid` and `owner_row`
from its detail to a copy of the same payload, then call the same frozen package's
`observe` entry and require that exact PID/incarnation. Root separately owns
machine installation, rental cleanup and the categorical policy isolation gate.

Source/static and CPU checks do not qualify GPU behavior. Automatic allocator
shortage asks, foreign-pressure sampling, cache/graph callbacks, broader container
namespaces and full P3 policy/TensorD retirement remain separate work.

A held-publication-lock Start refusal also remains a GPU fault-injection gate:
Start must fail closed without exposing partial `host.memory`; the failed
executor is retired by its existing lifecycle. CPU-only Hello does not claim to
prove a later CUDA Start failure.

`cleanup_failure` is a deliberately destructive **last** cell. Run it only after
the normal `gpu`/same-PID `observe` pair and its collector-after snapshot. It closes
the actual native participant, proves a checked Memory boundary refuses execution,
then unwinds `xs138_cleanup_original_error` through the real unchecked cleanup seam.
The expected run result is failure carrying that original sentinel; native cooperation
failure must also be reported/latched. A replacement cooperation exception is a red
result. This worker must not serve another qualification cell afterward. The normal
parity interval must not include this intentional owner closure.

## Physical shortage cells (not GPU-qualified yet)

`stage-shortage.py` archives a clean committed source tree into a new output
directory, builds the tiny GIL gate there, verifies its GLIBC floor and exercises
its real `PyGILState_Check`/pipe handshake without CUDA. Keep that fixture immutable.
The standard payload generator binds the actual Runtime/TensorFS wheels; add
`case` and `gil_gate_sha256` from the staged `fixture.json` for `/shortage`.

Run these cells on the owned headless rental: physical setup intentionally holds
most available VRAM in a live allocation. No managed cap substitutes for a real
physical refusal in positive cells; native driver-refusal counters must increase.
The fixture never manually asks or services a donation.

| Case | Required result |
|---|---|
| `working` | Local working/weight reclaim precedes automatic peer recovery; real allocation and retained values survive. |
| `weight` | Demand Region admission recovers after real driver refusals, with one successful fill and a valid phase sample. |
| `gil` | Real Torch `UntypedStorage.resize_` recovers while the donor holds its GIL inside the checked native gate. |
| `stopped` | A real SIGSTOPed/live donor remains Waiting with zero completed bytes; coordination expires, control stays active and exhaustion is clean. |
| `cap`, `va` | Explicit local limit or oversized VA request refuses with zero physical-create OOMs and zero peer recovery. |
| `limited` | No-reclaim Region admission refuses physical pressure without asking peers. |
| `capture`, `replay` | Guarded allocation refuses with zero native driver calls and zero peer recovery. |

Positive phase samples must publish without invalidation. FinalRetry means a
final local admission pass with its normal finite local retries, not exactly one
driver call. Allocation elapsed time includes CUDA work; the two-second bound is
on coordination. The GIL gate's ten-second guard is a failing harness outcome,
never evidence of donation. [`ctypes.PyDLL`](https://docs.python.org/3.12/library/ctypes.html#ctypes.PyDLL)
retains the GIL during its native call; the helper also checks it before signalling.

The frozen earlier `/gpu` and `/observe` remain the service-only parity control.
Standalone Rust, observed donor death, overlapping roles, stopped publication and
full initialized-GPU lifetime cases are separate required gates. These source and
CPU fixture checks do not close P3 or qualify game coexistence/cache callbacks.
