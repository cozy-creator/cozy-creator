# Ownerless graph-finalizer resources

The old graph retain_object(Definition) is invalid for automatic ownership:

    Torch finalizer registry -> retained holder -> Definition.owner -> Offload
      -> automatic states -> cache.segment -> GraphSegment._graph -> same graph

The holder's process-rooted bound finalizer therefore prevents its own target
from collecting. No caller reference is necessary. Explicit close/pressure could
break it, but neither is an acceptable condition for normal abandoned ownership.

The graph now retains a slotted `_Resources` payload. It has no Offload owner, module,
controller, cache, segment, graph, whole input/output container or TreeSpec.
It retains exact Versions (whose owner is only _Device, not Offload), immutable
Sources, OPEN Region wrappers and the dedicated
native Plan (which retains canonical Weight/Source/Region identity and layout),
pool/footprint metadata, consuming streams, last recorded tail and flat detached
Working Tensor storage owners. Detach preserves actual allocation/storage custody
while removing Python Tensor attributes, autograd/control links and tree contexts.
No ordinary weight Tensor/Pin is added to definition lifetime.

GraphSegment keeps the control Definition outside the retained holder. Ordinary
control cycles are collectable once caller references disappear. The finalizer's
resource branch cannot point back into that control cycle. Replay still creates
fresh normal checked exports and waits selected Source retirement before exclusion.
Existing stream ordering, Working/static/buffer records, pool-copy tail ownership,
native Pin/fence/quarantine and retryable Plan close remain unchanged. Explicit
close drops its resource reference only after the existing checked close path.
Failure/unknown-custody process roots remain intentionally retained.

## Real CPU collection control

`graph_resources_cpu.py` uses an actual adopted CPU model, immutable Sources,
actual CPU Working tensors and installed Torch2.14 `_RetainedCallbacks` with the
real weakref.finalize registry. No fake CUDAGraph/Region/Plan or CUDA execution is
constructed. The initial return-Definition payload ran red: the real Offload owner
remained alive. That receipt is preserved. The corrected payload collects control
owner/model while the holder is still rooted; exact Versions/_Device/source/storage
owners stay live until its independent real CPU target dies and holder fires.
The output has an actual Python control_owner attribute; detached resource wrappers
do not retain that back-reference. Storage values stay exact. Sources and Working
resources then collect with the holder. No explicit owner.close or pressure occurs.

```sh
python release-gate/suites/varena/torch-offload/graph_resources_cpu.py
```

This is Python ownership/collection evidence only. It does not execute CUDA graphs
or prove native GPU completion, memory reuse or tail safety.

## Prepared actual GPU control — UNRUN

```sh
python release-gate/suites/varena/torch-offload/graph_resources_gpu.py --device 0 --mode idle
python release-gate/suites/varena/torch-offload/graph_resources_gpu.py --device 0 --mode inflight \
  --provider /operator/matched/varena-tensor-export-provider.so \
  --provider-python /operator/matched/provider.py
```

Run only in root's guarded ordinary-Cozy child/cohort. These use actual automatic
graphs; no model close/reset or cache-pressure request is allowed to produce the
collection result. Weak owner/model/controller/cache/segment/graph/resource refs
must disappear BEFORE any trim. Inflight mode queues real replay/copies behind the
existing native mapped gate, collects on a checked host thread without opening
that gate, then requires native trim to keep pending backing owned. After release,
actual output parity, retirement completion and pool release must hold without
new quarantine. Whole-child protocol deadline remains root-owned.

The old exact-source automatic gate remains UNRUN/unqualified pending this patch
and actual GPU receipts. This source fix does not convert prior CPU/old-source
receipts into a CUDA/drop/performance pass. SDXL activation remains separate.
