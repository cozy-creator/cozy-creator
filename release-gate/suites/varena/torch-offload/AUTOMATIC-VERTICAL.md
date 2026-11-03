# Automatic dispatch and first nn application

The controller is wired through the public `vt.offload(..., graphs="auto")`
default. `graphs="eager"` disables automatic selection. CPU stays real eager
execution. The connected controller admits reviewed nn and exact SDXL framework
paths; `TORCH-AUTO-SDXL.md` records the 39 stock candidate boundaries and 44
effectful CLIP layer boundaries that remain eager. The small first application
below uses only nn classes, independently of the full stock SDXL gate.
Unknown/custom/effectful classes, aliases and unsupported input/output trees retain
the ordinary eager path. No warmup is authorized by a cold sample alone.

## Executed path

1. Select the existing nonoverlapping declared frontier and immutable invocation
   key. Record actual original/installed forwards during adoption. No whole-model
   graph is made from a composite fallback root.
2. An eligible cold call opens one native eager observation, executes its original
   body once, classifies its actual outputs, and waits the final4634 helper's OPEN
   `scope.checkpoint()` before closing the observation. Only a published native
   `successful_peak_bytes` is retained; alias/type refusals keep original results.
3. A later matching call opens an unpublishing preparation observation, creates a
   private pool and passes that exact native peak unchanged to `prepare_pool`.
   The admitted SAME pool reaches the private GraphSegment constructor BEFORE its
   first authored warmup. Static inputs/weights, scoped Use readiness and actual
   capture remain the existing checked frontend path. No multiplier/estimated cap.
4. A completed definition is published to its existing native CacheOwner. Matching
   calls use actual cached replay with fresh checked weight exports and readiness,
   replay_scope around launch only, extra pool custody through copy tails, and
   independent ordinary output copies. Automatic output copies preserve the cold
   caller's normal versus inference Tensor mode; explicit GraphSegment is unchanged.
5. Native pressure requests retire only idle completed entries through their
   nonblocking admission/graph locks. The callback takes no model execution lock;
   logical success is not byte credit. Native trim owns physical release/accounting.
   Key changes retire the old definition only when its tail is ready. One current
   key per boundary, no unbounded retired-definition list, no shared private pools.
6. Close stops admission under the owner lock, drains native callbacks outside
   model/entry locks, then closes exact definitions and native observation keys.
   Native/consumer/capture uncertainty retains exact execution owners and fails
   closed. No forced Pin release or guessed completion.

Only the exact pre-body `prepare_pool` typed OutOfMemory/Exhausted refusal can
choose eager once. Its preparation observation and idle entry lock are released
before that body starts. Body/capture/replay errors never choose another execution.
Cancellation refusal propagates and retains custody. No broad RuntimeError catch,
message parsing, reserve resizing, application retry or allocator byte policy.

Known same-owner enclosing intent is suspended/restored by the existing epoch
owner outside capture. Native typed `PhaseOverlapError` at observation ENTRY alone
chooses eager under the caller's existing observation, without closing/relabeling
its token/sample. Catching this type does not wrap a body. Current implementation
dependency is the root-owned typed-refusal feature83ebdad55 (equivalent mappings
2bf8fd329/dcbc141b2); root supplies the matched native/PyO3 cohort. The automatic
observation path catches the public `vt.PhaseOverlapError` alias directly. Older SDKs are unsupported,
and reject before entry/body rather than using capability/empty-tuple fallback.
Other native admission errors on the required SDK still propagate unchanged.

`owner.graph_facts()` returns passive reasons/counters/native keys and peaks, not
graph/Plan/Region/tensor handles. Scope counts and Python facts are not GPU proof.

## Real two-block application

```sh
python release-gate/suites/varena/torch-offload/automatic_two_block.py --device cpu
# Coordinator only; required matched typed native SDK:
python release-gate/suites/varena/torch-offload/automatic_two_block.py --device cuda:0
```

The actual two sequential blocks contain Linear/ReLU/Linear. No mocked native
resources, injected allocator methods or caller reserve are used. CPU mode checks
the actual public adoption/controller/eager path and remains CUDA-uninitialized.
It does not simulate or qualify GPU graphs.

GPU mode requires: original cold bodies once, native peaks, promotion and cached
replay without Python bodies; independent normal outputs and exact numerical
parity; both definitions alive with weight pins0; real trim/refill at the same
Region addresses under unchanged definitions; native callback retirement of idle
entry1 while the model and entry0 admission locks are held; actual pool-byte
reduction after native trim; recapture of entry1 only. Mandatory typed controls
require one eager body under the caller token. They include named/unkeyed
parents and changed shapes, then ordinary cached selection resuming after the
caller closes its token. Quarantine may
not increase. Python profiling observes real forward calls and installs no hooks.

GPU mode is PREPARED/UNRUN by this lane. Actual source/epoch/reset/failure and stock
SDXL selection/performance remain coordinator-owned gates. The framework profile
is connected in the same SDK; the small nn app does not qualify stock execution.

No arbitrary phase-enter RuntimeError is classified. phase_stats and split_context
are not phase-admission proof and are unused for overlap classification. Unknown native entry
errors propagate before yielding to any body. Only the separately owned typed
error can authorize parent-observation eager selection.

There is no parent-control bypass flag or alternate ABI path. The matching native
typed class and scoped Use readiness are mandatory. Historical receipts remain
preserved without keeping their obsolete execution paths.
