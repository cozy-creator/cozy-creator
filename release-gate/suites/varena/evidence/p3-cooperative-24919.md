# P3 explicit GPU cooperation: run 24919

**Passed:** all nine groups, `Report.passed=true`, nested `passed=true`, child exit 0.
The [complete group receipt](p3-cooperative-24919.json), [unmodified run record](p3-cooperative-24919-run.json)
and [provenance/input](p3-cooperative-24919-provenance.json) are preserved.

Native TensorFS source/wheel is `1c6b1c15b3c6e5929e29de5cfcb997312219afd5` /
`0.17.16+dev.g1c6b1c15`; fixture source is `e25a46b751f15ec2f8c27e8a1373621cd1ee6000`.
Only two release-gate files changed between those heads; no native or Python product source changed.
Runtime is `0.36.18+dev.g97706e71`, TensorD `0.15.4` / `9d2ae4827`.
The run used the ordinary Cozy entry point on `ghislaine`, one RTX 4090,
UUID `GPU-2eb9e2ca-fca5-d50d-4344-7d0127c4ec68`, with exact extension/shim/Torch checks in each fresh child.

The cooperation domain was **same effective UID, user namespace, PID namespace and mount namespace**,
one host boot, and the same real tmpfs inode. This is an explicit native mechanism result, not an
automatic memory-policy or TensorD migration result.

| Group | Actual observation |
| --- | --- |
| `completed_exchange` | Donor physical holdings fell from 67,108,864 to 16,777,216 bytes. Successful release/ack was 50,331,648 bytes (48 MiB). It retained 8 MiB live working and 8 MiB pinned weights. Driver-used bytes independently fell by exactly 48 MiB; the requester then allocated, touched and checked 32 MiB. The pre-release sample was rejected for a fresh ask, and duplicate service released zero. |
| `pending` | With the real consumer event incomplete before and after service, release/completed stayed zero and pending was 48 MiB. That service took 0.198 ms in this one observation; this is not a general response-latency claim. After the owner completed its GPU work, service released 48 MiB and the complete consumer output was checked. |
| `unknown` | An actual native DLPack final owner remained Unjoined and retained its backing; peer service made no context join or event wait and did not promise unknown consumers as pending release. Other eligible backing still supplied 48 MiB. |
| `capture` | Actual graph capture and replay-guard service deferred with zero released bytes and zero native CUDA-call delta. Explicit close/open during capture were refused without losing the owner or querying CUDA identity. |
| `owner-drop` | External arena/region owners dropped on another thread during the measured service interval; 48 MiB was released and unrelated GPU work remained pending. The tick did not become final arena teardown or context-join that work. |
| `explicit_close` | Actual managed bytes reached zero and reopening obtained a new row incarnation (1 → 3). |
| `gc_during_capture` | A later native cleanup sentinel completed while capture was still active, then the other process observed the GC'd participant's row still retained. After capture, managed bytes reached zero and a new incarnation (3 → 4) opened. |
| `retirement_capture_exclusion` | A concurrent capture attempt **actually observed refusal** during retirement; this arm was not skipped or inferred from timing. Final managed bytes were zero. |
| `owner_death` | The real donor was killed during an unfinished round. Its answer became `OwnerGone` with zero completed bytes. Subsequent driver observation and a successful checked 32 MiB allocation were recorded separately; reap itself credited no VRAM. |

## Preserved first failure

Run **24917 failed**. Its [unmodified record](p3-cooperative-24917.json) and
[original NDJSON](p3-cooperative-24917.ndjson) remain intact, including the bounded/truncated log lines.
It reached the GC-active handshake after completing the earlier groups, then the child returned a blank
AssertionError when finishing GC capture. The fixture had recorded `torch.ones` into a graph and read its
output before replaying it. Recording does not execute the fill.

Fixture-only `e25a46b75` adds `vt.replay(graph, stream=stream)` before the existing synchronization/read,
preserving pool/consumer recording. It also preserves full nested errors, partial receipts and child exit
in the typed result, because log text alone is truncated. A completed CLI/run now must still have
`Report.passed=true`; 24919 explicitly did. The same native wheel ran both attempts.

## Boundary

No automatic allocator asks, autonomous sampler, Python cache/graph owner integration, model coexistence,
foreign-game latency or TensorD byte-policy cutover is established here. Cross-container/private namespaces,
MIG, multi-GPU and raw Rust Drop inside capture remain unqualified. Pending partial-release accounting was
reviewed and retains completed effects across later errors; this run did not force the specific post-release
Busy/snapshot race. P3 as a whole remains incomplete.
