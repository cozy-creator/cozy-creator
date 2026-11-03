# Managed component entry wiring

This delta composes the final, unchanged component helper into actual adopted
entry points. It does not implement automatic graphs or claim a performance win.
The separately delivered helper/client files remain byte-identical to their owner.

The adapter records the actual original and final installed forward for every
adopted module. Reviewed declared forwards and supported nn containers open a
component scope; exact reviewed stock UNet2DConditionModel, CLIPTextModel and
CLIPTextModelWithProjection roots open an outer scope. A known same-owner nested
scope reuses the selected stream. Actual changed streams keep the helper's safe
fallback and cannot train a cold measurement. Unknown custom root/callable output
protocols keep the earlier per-operation execution path. These eager stream scopes
execute authored code once and do not establish graph-purity eligibility.

Stock AutoencoderKL.encode and decode receive explicit method wrappers. They
use the reviewed original methods and actual opaque return leaves; no forwarding
through root.forward, posterior sampling or container reconstruction occurs.
Slicing/tiling, Accelerate hooks, unknown subclasses/method implementations decline
the new method scope before body and use the existing eager path. No capture
failure or body error is retried. The supported result object and all aliases are
returned identically. Normal tuple/None fields remain unchanged.

The existing `_Device.execution`, ordinary operand `record_stream` loop, native
weight export/Pin/fence paths, intent and conversion code are unchanged. Frozen
CaptureBindings supply their authoritative stream and bypass eager joins/records/
witnesses. No dummy native lease, graph-lifetime weight export or byte policy is
introduced. Host `component_facts()` reports root/method scope-entry counts only;
it is neither GPU completion nor graph/performance evidence. Descendant intent
counts remain in the existing intent facts rather than duplicate scope counters.

The real SDXL component client now verifies automatic outer entries: two text
calls per encoder, two UNet calls, three encode and three decode calls. It still
checks the actual178/83 frontier,39 candidates/44 collector-effect refusals,
eager/adopted parity, unchanged RNG, VAE object identity and public config changes.
All tensors execute on CPU in the recorded gate; CUDA remains uninitialized.
The existing real CPU scope/binding/alias/direct-read client and alias/effect gate
are rerun with the shared wiring. No fake stream/event/native resource is used.

```sh
python release-gate/suites/varena/torch-offload/sdxl_components.py --device cpu
# Operator only; prepared/unrun. Installs before ALL baseline CUDA allocations:
python release-gate/suites/varena/torch-offload/sdxl_components.py --device cuda:0
```

GPU event/dependency/tail lifetime controls from the helper owner, actual capture
and repeated stock-model timing still require the coordinator's matched cohort.
The GPU mode above is an eager component gate, not automatic replay/performance.
Native typed refusal dcbc141b2 is source-composed by root as2bf8fd329/current83eb;
the locked offline source check passes. Matching built-extension dispatch and
actual overlap GPU matrix remain pending with their assigned owner. This is no
longer an unimplemented seam. The adapter catches exact public vt.PhaseOverlapError
only at entry; unrelated errors propagate unchanged, with no text/stats inference.
