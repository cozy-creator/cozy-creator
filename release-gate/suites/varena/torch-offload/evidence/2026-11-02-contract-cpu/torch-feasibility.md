# CPU Torch offload feasibility

Executed three standalone probes with the supplied **Torch 2.14.1+cpu**, one Torch
thread and one interop thread, `CUDA_VISIBLE_DEVICES=''`. Every probe asserts CUDA
is uninitialized and no `libcuda.so` is mapped. No package installation, model download,
Varena/native allocator simulation, GPU work or product-source edit occurred.
Exact commands and hashes: [feasibility-manifest.json](feasibility-manifest.json). The backing tensors in
these experiments are real CPU tensors; the experiments prove only Torch semantics.

## Demonstrated positive behavior

- A metadata wrapper Parameter can replace a plain Parameter via `swap_tensors` while
  retaining the original Python object and both tied module registrations. Real CPU
  Linear inference and direct functional access agree exactly (`[[10, 28]]` for the
  two tied Linear calls). Default parameter enumeration gives one object; disabling
  duplicate removal gives both paths.
- Managed transpose/narrow/detach aliases preserve shape `[2,2]`, stride `[1,3]` and
  offset `1`; their values remain valid after the module/original references are dropped.
- Schema write-argument checks reject managed `add_`, view `copy_`, `out=`, `set_`
  and `resize_` before the CPU payload changes. Ordinary clone/cast results remain
  plain mutable tensors; writing an ordinary `out=` destination is supported.
- A controlled conversion scope returning managed `_to_copy` results lets standard
  `Module._apply` preserve tied Parameter identity for a float32→float64 conversion
  **when no escaped view prevents swapping**. No global Torch future flag was changed.
- A `__torch_function__` guard catches `.data =`, `.requires_grad = True` and
  `.requires_grad_(True)`. A `.data` read remains a managed alias whose writes are
  rejected; `.clone()` remains an ordinary mutable copy.

See [torch-probe.json](torch-probe.json) and [torch-guard-probe.json](torch-guard-probe.json).

## Red controls and limits — preserved, not acceptance passes

1. **Stock `_apply` with an escaped managed view fails.** A transpose view raises the
   original Parameter's TensorImpl use count to 3. The ordinary swap path fails with
   `_apply(): Couldn't swap Linear.weight`; its cause is `swap_tensors` use-count
   refusal. The full traceback is retained in `probe.json`.
2. **Schema-only write rejection is insufficient.** Real
   `aten.native_batch_norm(..., training=True, ...)` has no write annotations on its
   running-stat arguments. It mutates guarded running mean from zeros to
   `[2.25, 2.75, 3.25]` and running variance from ones to `[8,8,8]`. This is an executed
   counterexample, not a simulated backend. Preserve explicit training rejection and
   known semantic exceptions; do not claim a universal immutable guard from schemas.
3. A wrapper disabling `__torch_function__` accepts `.data =` and enabling gradients
   without any `__torch_dispatch__` call. `.data =` can change outer tensor state while
   leaving the Python payload unchanged. Guards must cover this separate boundary.
4. Existing weakrefs refuse `swap_tensors`. A pre-existing ordinary CPU view also
   refuses swap (use count 2 on a non-gradient tensor). Blind swap moves the original
   Parameter's custom `__dict__` attributes to the replacement object, despite retaining
   the Parameter's Python identity.
5. Eval mode and frozen managed Parameters do not prohibit autograd: a real input
   with `requires_grad=True` produces a gradient-bearing output and input gradient
   `[[6,10,14]]`. The dispatcher sees grad mode true in this forward and false in its
   backward. Do not silently detach training/gradient requests.
6. A real `ReadOnlyTensorWrapper` DLPack export imports as an ordinary mutable tensor;
   changing it modifies the source storage. A DLPack read-only flag is not an ordinary
   Torch immutability barrier in this tested cohort.

See [torch-boundary-probe.json](torch-boundary-probe.json). Probe records labeled `pass` sometimes
mean the probe successfully reproduced a counterexample; they do not certify an offload
adapter. `module_apply_with_escaped_view` remains explicitly `observed_failure`.

## Exact guard/rebind sketch demonstrated

The experimental wrapper keeps `__torch_function__` active. It rejects the data and
requires-grad setters and `requires_grad_(True)` before invoking the original function.
Other functions run under `DisableTorchFunctionSubclass`, letting real ATen calls reach
the wrapper's `__torch_dispatch__`. That handler rejects schema writes to managed arguments,
unwraps real CPU payloads, preserves managed aliases with `return_and_correct_aliasing`,
and returns independent results as plain tensors.

The CPU-only controlled rebind experiment uses a private context-local capability:

```python
candidate = ManagedWeight(converted_real_cpu_tensor, new_version)
with internal_rebind_capability:
    original_parameter.data = candidate
    original_parameter.payload = candidate.payload
    original_parameter.version = candidate.version
```

This preserved Parameter identity and ties, while an escaped old view retained its old
float32 dtype/storage/value and the Parameter became float64. It worked both without and
with the Python setter guards. It is a **feasibility result**, not permission to use
arbitrary `.data` mutation or a finished native conversion protocol.

Production still needs agreement on exclusive/transactional conversion, preservation of
user Parameter metadata, native Source/Region versions, old-view owners, failure rollback,
explicit stream ordering, and active capture/use refusal. The experiment does not establish
autograd, compile/graph, optimizer, arbitrary subclass/custom-op, concurrent rebind or CUDA
safety. It does not prove residency, pins, final storage deleters, producer/consumer fences,
quarantine, asynchronous Source publication or physical reclamation. Those remain native
contract and GPU qualification work. No full operator audit was attempted.
