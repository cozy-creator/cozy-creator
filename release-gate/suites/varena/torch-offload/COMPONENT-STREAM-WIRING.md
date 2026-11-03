# Minimal committed component-helper integration

Helper feature18e44b9715c31483b5fac201291fe41e1c33675e is composed unchanged (mapped
commit87e8c4e9b). Its `_offload_stream.py` SHA256 is
`1d817120156e779c79d1387a70bdf5da86919b554180af7284be28a08e71e26d`.
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
aliases, failed prefix/final witness refusal, closed checkpoint refusal, direct managed
read and actual frozen `_CaptureBindings` with no new materialization/eager witness.
It calls actual tiny Diffusers VAE encode/decode under explicit scopes, preserves
the return objects/leaves and exact values with root.forward callcount0. No fake
CUDA stream/event/owner is used. These are CPU/host semantics, not CUDA reuse proof.

Source checks preserve AST of `_Weight`, `_Version` and `Offload`; the two ordinary
record loops remain in fast/fallback paths. The helper equals its source18e Git blob.
Ruff/format/syntax/diff pass. Matching native images are existing audited host artifacts,
not a newly built or GPU-qualified current paired SDK. No native rebuild or GPU run.

Root must bind BOTH helper and wired `_offload.py` hashes in the Python overlay/SDK;
adding only the shared source would leave its lazy helper import unavailable. Do not
adopt contextee7c9c025 (only qualified eb315 `_offload.py` hash2597ec31) or another native
snapshot; compose only original/mapped helper18e and the subsequent shared wiring delta.
The helper owner supplies final client/receipt commits separately. Its prepared GPU
context-count/changed-stream/alias/exception/capture/VAE gates remain operator-owned.
No speed or automatic execution, allocator policy, rental, machine, merge or release
claim accompanies this wiring stage.
