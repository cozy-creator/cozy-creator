# Up-block-0 asynchronous diagnostic helper

This is the single tracer reused from run27056's `allocation_trace.cpp` (source
SHA256 `c8989df302143bd9ff5d3937391e62212714b32b41930bff0ec58b8a39d37eb7`).
It delegates the installed Varena allocator and subscribes to its existing
`AllocatorTraceTracker`; it adds no allocation policy or GPU synchronization.
The numeric client remains `e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652`.
The frontend diagnostic wrapper is owned separately and must load only this helper.

`diagnostic_markers.h` is the shared marker contract. IDs100–203 describe host
entry/return/error observations. A cache retirement return is logical only.
`graph_identity` is a diagnostic/Python control identity, not a CUDA graph handle;
`plan_handle` is an ABI handle, not a native Plan ID. The wrapper must attach explicit
reasons for unavailable0 identities. It joins native owner IDs through actual
admission logs and safe binding metadata, never by substituting one numeric ID for another.

## Window and output

After CPU-only Varena binding, call `allocation_trace_install()` once before CUDA
initialization. Open `allocation_trace_window(1)` for up0 invocation2 and keep it
open through invocation3 and the following boundary/error. Select ATen recording
only for the target invocations using `allocation_trace_stage(nonzero)`; stage0
disables ATen callbacks while primitive native allocation/segment/marker records
continue across the gap. This does not execute or retry a model body.

At outer completion/error, outside capture/replay/native exclusions, disable the
ATen stage, close the native window, and call `allocation_trace_dump()` independently
of phase-stats/snapshot success. Never suppress buffered rows because the allocator
is poisoned. The helper's dump makes a bounded metadata copy under its recording
locks and serializes only after releasing those locks. Snapshots remain later,
post-lock observations; their ordinal cut does not make them allocation-decision facts.

Format3 is self-describing. The old17 row columns are preserved; five columns are
appended: `monotonic_ns`, `marker`, `invocation`, `graph_identity`, `plan_handle`.
The1-based row index is the common committed ordinal returned by the marker API.
Time is sampled at commit from C++ `steady_clock`; `thread` is the process-local
native thread-id hash. These order host observations, not GPU execution.

Kinds1–14 retain their previous meanings. Kind15 is `working_segment_mapped`,
observed after native `Driver::map` succeeds. Kind16 is
`working_segment_retire_intent`: the existing `SEGMENT_FREE` notification occurs
BEFORE unmap/release and is not completion evidence. Kind17 is a host marker.
The segment observer supplies no stream, so its stream0 is unavailable; its pool
pair{0,0} means ordinary eager backing, not a private graph pool.

The ring retains131,072 rows and24 optional snapshots without overwrite. Overflow
sets `lost=true` and increments `dropped_rows`; invalid markers, unsupported observer
actions and unavailable snapshots are explicit. A diagnostic with loss is incomplete.
Final coverage also requires the intended invocation/marker sequence and a closed
window; an intermediate dump with an open window is only a prefix. There is no
completed Region or Working release hook in this version.

## Build and host boundary

`build_helper.py --out <task-output> --cuda-crt-include <matching-CRT>` builds only
this diagnostic `.so`, using existing Torch2.15.0+cu132 at commit
`d0f2af8b66b20da4baa50644c0cd3f887a2eae2e`. It checks that CUDA remains uninitialized.
Root owns the exact execution cohort; development context is not the execution base.

`host_contract.py --helper <so> --mode markers|overflow` uses a real qualified
TensorFS wheel payload, its actual observer registration, real marker calls and
CPU ATen operations. It creates no CUDA allocation and synthesizes no allocator
event. These checks cannot qualify GPU segment events or reproduce719. The controlled
guard wheel's tested native getter supplies the actual cached-workspace flag; no
second allocator or mangled ctypes symbol is loaded to query it.

The first model diagnostic stays asynchronous. Additional stream checkpoints,
launch-blocking or native completed-release hooks require a separate agreed control.
