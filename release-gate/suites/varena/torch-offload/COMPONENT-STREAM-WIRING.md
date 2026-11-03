# Minimal committed component-helper integration

Helper features18e44b9715c31483b5fac201291fe41e1c33675e and
d9e6906648ff840f104354721fa9028fa0e0a165 are composed unchanged as87e8c4e9b/9fbf30054;
host fixture correctiondacc069b2 is composed as78f70ff70. Final helper SHA256 is
`4634606ba2954f69884d86d1a0c591a7bfc909f0dc181153d9c310eed0e167e0`.
The initial18e hash1d817120 is historical; its earlier receipts are not relabeled.
The separately owned new helper/clients remain untouched; this lane changes only its
owned `_offload.py` execution seam plus this lane's own real client/evidence.

The CUDA `_Device.execution` branch retains compiler/tracing, device and capture
validation BEFORE reuse. Query the owner device's actual capture state, not an
unrelated current device. Reuse only when that current device and selected explicit
scope match; a changed stream is still observed and uses the old safe context path.
Both paths keep the actual per-operation ordinary-input record_stream loop. All
materialized weight records, exports/Pins, checked fences and direct read behavior
are unchanged. Capture dispatch still selects the frozen binding path instead of
eager `_Device.execution`; no helper eager joins/witnesses occur there.

This stage enables the existing explicit `component_scope` contract. It does NOT add
automatic root-forward/VAE encode/decode wrappers or default automatic graph dispatch.
Do not claim those entrypoints were automatically intercepted. In particular opaque
VAE outputs require exact explicit tensor leaves as in the helper CONTRACT.md; the
scope returns the identical object and must not sample a distribution merely to find
its storage. Unknown opaque CUDA outputs must not be silently mapped by new wrappers.

Real CPU client `component_stream_wiring_cpu.py` covers adopted model exact values,
nested host scopes, active checkpoints/host-only no-training, output object/Storage
aliases, independently completed prefix surviving later failure, invalid failed FINAL
completion, closed checkpoint refusal, direct managed
read and actual frozen `_CaptureBindings` with no new materialization/eager witness.
It calls actual tiny Diffusers VAE encode/decode under explicit scopes, preserves
the return objects/leaves and exact values with root.forward callcount0. No fake
CUDA stream/event/owner is used. These are CPU/host semantics, not CUDA reuse proof.

Source checks preserve AST of `_Weight`, `_Version` and `Offload`; the two ordinary
record loops remain in fast/fallback paths. The helper equals its final dacc Git blob.
Ruff/format/syntax/diff pass. Matching native images are existing audited host artifacts,
not a newly built or GPU-qualified current paired SDK. No native rebuild or GPU run.

Root must bind BOTH helper and wired `_offload.py` hashes in the Python overlay/SDK;
adding only the shared source would leave its lazy helper import unavailable. Do not
adopt contextee7c9c025 (only qualified eb315 `_offload.py` hash2597ec31) or another native
snapshot; compose the original/mapped helper feature commits and shared wiring delta.
The helper owner's final committed clients/diagnostic are composed. Its prepared GPU
context-count/changed-stream/alias/exception/capture/VAE gates remain operator-owned.
No speed or automatic execution, allocator policy, rental, machine, merge or release
claim accompanies this wiring stage.

Final CPU evidence is in `evidence/component-stream-final-composition`. The original
extra no-libcuda failure and separate Torch-import driver control remain in the helper
owner's outputs with hashes recorded there. CUDA-enabled Torch may map libcuda during
import while tensors remain CPU and torch.cuda remains uninitialized. Final clients
check unchanged mapping across CPU execution, not whole-process driver absence.
