# Frozen-core lifetime diagnostics

Owner: `/root/p3_owner_restart`. Product core remains08a0d955e/coref0cf14dd;
only fixture helpers/controller change. Preserve original runs26035,26036,26049,
26054. No original predicate,10-second deadline or assertion is weakened.

- Async fixtures now emit bounded stderr stages before/after resource context
  entry, each stream destruction and pinned-host free. Existing predicates emit
  their label and exact Fill/Arena facts at success/refusal. This identifies which
  original predicate failed and whether a resource callback returned.
- C `host_bytes` emits stages bracketing public Gpu open, region construction,
  allocation, use, done and close. It preserves the closed unrelated gate and
  original60-second parent protocol deadline.
- `owner_try_close` now observes its actual service table through `Ledger`, which
  registers no participant. Its pre-existing manual CUDA-call counter Peer lives
  in a separate private counter table; no extra automatic publisher or relaxed
  one-participant guard is introduced.

The leading hypothesis is pinned-host allocation/free implicit synchronization,
not an established blocked-call finding. NVIDIA documents that cudaFreeHost can
cause implicit device synchronization, while general CUDA calls may block for
internal resources. The exact stage traces and low-level experiment must decide
before any product pinned-memory policy changes.

## Bounded pinned-host API diagnostic

Build `pinned_host.c` as `varena-pinned-host` with C11, `-Wall -Wextra -Werror`,
`-ldl -pthread`, in the same named portable compiler image. It dynamically loads
the actual CUDA Driver and verifies the supplied UUID; it does not link another
Varena core or call a device-allocation API.

Run through the ordinary controller's `pinned_host_alloc`/`pinned_host_free`
entrypoints using the exact helper manifest/payload. Internally:

```text
varena-pinned-host alloc GPU-actual-UUID 0
varena-pinned-host free  GPU-actual-UUID 0
```

The gate is a distinct4-byte mapped pinned-host allocation. There is one CPU
writer using an aligned lock-free release-store and one GPU read-only EQ wait;
no GPU atomic or payload-publication inference is made. The actual device pointer
comes from cuMemHostGetDevicePointer. Before experimentation, a CUDA event after
the stream wait must complete from the CPU store alone. This proves the mapping
and gate path on the actual device. All resets happen after that event completes.

The tested cuMemHostAlloc/cuMemFreeHost runs on a separate owned host thread with
the same retained primary context. The controller observes entry and a100ms
window, then releases the gate with a CPU store only—never a CUDA call that could
wait behind the tested API's driver lock. API return and the stream's event are
checked separately. Both early return and return after release are recorded;
elapsed time is not itself a deadlock, death or cleanup-success inference. The
10-second observation bounds retain failed ownership for isolated process exit;
the existing parent60-second bound remains unchanged. Cleanup occurs only after
actual return/event completion, with no context-synchronize workaround.

Primary contracts:
- [CUDA stream memory operations](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__MEMOP.html): device pointers may refer to mapped host storage; the event makes stream-wait completion explicit. Do not rely on an invisible ordering between two GPU tasks.
- [Mapped host memory](https://docs.nvidia.com/cuda/cuda-programming-guide/02-basics/understanding-memory.html): naturally aligned scalar accesses are preserved; this probe does not assume host/device atomic read-modify-write interoperability.
- [NVIDIA capture failures](https://docs.nvidia.com/dl-cuda-graph/latest/troubleshooting/capture-failures.html): pinned-host free can implicitly synchronize the device.
- [CUDA synchronization behavior](https://docs.nvidia.com/cuda/cuda-runtime-api/api-sync-behavior.html): API blocking behavior is not a general nonblocking guarantee.

Build helpers in a NEW copy of the operator's final004 stage mounted at the same
`/work` path, overlaying only these fixture paths. Reuse the exact final004 Cargo
feature/package command and compiler; hash core and extension before/after and
require equality. Never replace the installed core or wheel to make helper parity
pass. Copy resulting helpers to a new artifact directory, record their hashes,
and regenerate manifests/inputs. Operator owns compilation scheduling and GPU
execution. A source/check pass is not a diagnostic result or product fix.

## LM26079 event-count diagnostic

The original result has6 live allocations on both sides, identical live/capacity/
mapped/free bytes, pending0 and reclaimable0, but events36→54. Native take keeps
block.events; completed poll changes reuse state without removing that map. A
live allocation can keep its eager segment resident through trim. This mechanism
is ranked first; the one deliberately live pool may also retain its own event
map. The aggregate public counter cannot attribute the exact18 by itself.

`event_history` is a separate real native memory probe: one live512-byte anchor
keeps eager backing resident while the same temporary block is allocated/freed
on18 distinct, still-owned streams. Each stream completes before polling/trim.
There are no graph pools or caches, so retained events there belong to blocks.
It records the complete byte/event baseline, then retires only those completed
fixture-owned streams and checks event cleanup while the live anchor remains.
No freed pointer is read; the still-owned anchor is checked before final cleanup.
This is mechanism evidence, not automatic attribution of the LM's18 handles.

The staged LM overlay is based on23dbb8db8 and adds only read-only domain stream,
pool-tag, graph-count and engine-owner facts. Every original assert AST is equal,
including strict event_handles equality; it does not close the intentionally
live domain to manufacture a baseline. Apply that one file to a NEW23db project
copy, not to a product/Runtime tree. GPU diagnostics remain unrun.

## Confirmed pinned-host result and modern API probe

Leg17 runs26082/26083/26084 identify cuMemFreeHost as the blocked call:
free returns only after the CPU-only gate opens, and both original async resource
callbacks stop at before_host_free after destroying their producer streams.
Run26085 reaches public host_bytes.before_done; it does not implicate allocation.
Run26087 qualifies the registration-free owner_try_close fixture correction.

`modern_host.c` includes the official pinned CUDA13 header; no hand-written pool
or VMM property layout. Build with C11, -Wall -Wextra -Werror, official include
path, -ldl -pthread. It also includes pinned_host.c for the exact prior gate and
bounded observation helpers. No Varena code or installed artifact changes.

The `modern_host_pool` and `modern_host_vmm` entrypoints run separate children:

```text
varena-modern-host pool GPU-actual-UUID 0
varena-modern-host vmm GPU-actual-UUID 0
```

Both report HOST/HOST_NUMA pool attributes. An unsupported capability is an
explicit diagnostic result (`supported=false`, `qualified=false`), never a
successful modern-staging claim. HOST pool has maxSize8MiB for one4MiB allocation.
VMM uses HOST, handleTypes0, actual granularity, and both CPU/GPU access grants.
The full CPU write -> H2D -> D2H readback completes before the unrelated gate.
The allocation's only actual transfers have completed before release starts.

A separate owned thread releases via freeAsync + an actual completion event +
poolDestroy, or VMM unmap + release + addressFree. Individual stage markers and
completion/return timestamps distinguish queue admission, real stream completion,
and final destruction. The100ms closed-gate window is an observation, never a
failure inference; the CPU alone opens the gate, preserving the same10s/60s
protocol limits. No context/device synchronization is inserted. One successful
sample cannot prove all hardware support or a universal nonblocking guarantee.

Primary references: [current stream-ordered allocator](https://docs.nvidia.com/cuda/cuda-driver-api/cuda_driver_api/group__CUDA__MALLOC__ASYNC.html)
and [CUDA13.0 VMM](https://docs.nvidia.com/cuda/archive/13.0.0/cuda-driver-api/group__CUDA__VA.html).
The actual pinned official header is source evidence for HOST attributes144/142,
HOST pool maxSize and HOST VMM properties; the actual driver decides support.

The product choice remains pending these probes. Existing TensorFS device I/O
already returns completed pinned slots to its process-owned bounded32MiB reader
engine. Direct Varena reuse would create a dependency cycle. Factoring a bounded
lease primitive down is possible but must preserve the measured reader-local
rings and avoid unbounded pinned retention or freeing on the retirement worker.

The successor additionally measures target event/stream destruction before the
unrelated gate opens. Only the unrelated gate's event/stream/pinned word are
cleaned afterward. HOST VMM granularity status/value are always reported; that
value is not silently assumed to be the stream-ordered pool's granularity.

`modern_host_pool_small` measures the exact2MiB host-Source stage with maxSize2MiB,
without silently rounding up. Both pool modes report actual reserved/used bytes
after allocation completion and after free completion. A granularity/cap refusal
is preserved as such; VMM granularity is not substituted for pool evidence.
