# Cache-entry retirement implementation: native GPU behavior remains unrun

_GraphCacheEntry uses the EXISTING logical _runtime.CacheOwner. It retains the weak
callback target and one graph only, holds admission throughout use, touches/suspends
requestability and declines a busy entry instead of waiting for the allocating owner.
Callback executes only under native CacheOwner producing/external-callback exclusion.
Private graph retirement checks real tail completion and its own graph lock, NEVER
model execution lock. Native ready-only trim owns all physical accounting/release;
callback success is logical retirement, not a byte credit/victim algorithm.

Existing public GraphSegment close retains its owner-lock order through extracted
same close actions. Capture/replay/tail/validation AST is unchanged from exact eb315
context. Callback Busy/capture refusal keeps exact retryable refs. Unknown tail/reset/
cleanup failure retains graph/epoch ownership and fails cache. No external Plan or
shared Region is closed; private cached graphs are not public execution handles.
Registration drain is outside entry/device locks, after stopping admission. Passive
facts cannot block on the admission being observed. Full controller wiring remains.

Checks here: five actual CPU key/effect/Storage/binding/Conv model clients exit0 on
exact source overlay, plus Ruff/format/syntax/AST/diff and real SDK --help. No native
CacheOwner or graph pool is constructed in those CPU controls. They do NOT qualify
private cleanup, callback refusal, concurrency, pool bytes or complete owner teardown.

Prepared GPU automatic_cache_pressure_gpu.py uses actual native pressure requests and
matched C mapped gate: active entry/admission plus model lock remain held with actual
GPU work pending, idle sibling must logically retire without that global lock, native
trim must remove its pool while active stays owned, outputs/rebuild must remain exact.
It tests real event/callback/pool objects, no fake resources or injected owners.
The complete GPU cell is UNRUN; reset/progress may fail/block and must retain its
failed receipt under the coordinator's explicit whole-child deadline. No speed or
whole-model/cache-pressure correctness is asserted from source checks alone.

The only context commit adds qualified eb315 _offload_graph.py for a minimal delta;
root skips that context and adopts feature-only changes. _offload.py and local stream
helper remain untouched. No native policy, GPU/rental/machine/merge/release actions.
