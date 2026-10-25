# Public native quantization caller lifetime

This real default-CLI cell writes an actual TensorFS checkpoint, converts it through the public
Runtime facade into **two separate declared output lanes in one request**, adopts both outputs,
checks their primitive producer bytes and stored integrity, then submits one independent real
CUDA PreparedRepeat through Context.plan. It requires both conversions to have joined without
consuming the request's one whole GPU-plan slot. No model mock, allocator fake or injected error.

```
cozy run /absolute/worktree/release-gate/suites/execution/native-quantization-caller/gpu seed=180301 --on local --gpus=1 --json
```

Use the installed Runtime0.32.3/TensorFS0.15.4 candidate only after root qualification approval.
This fixture changes no SDK or rental. Its installed result is recorded below. Native Writer basic/cancel/cancel-write
are separate required gates. An ordinary authored `cozy model quantize` recipe and consuming-model
proof remain required before production caller cutover. No performance claim.

The controller must also inspect the actual run telemetry: two `quantization measurement` rows,
outputs `int8` and `nvfp4`, each encoded_keys=1/reused_keys=0. The helper also emits
`native_quantized_tensors`, but the currently retained protocol does not expose that optional metric. The8×256 F32 source weight is8192bytes and the normalized8-element
bias is32source/16output bytes. Expected source reads: int8=8224, NVFP4=16416 (both passes).
Expected new bytes: int8=2096, NVFP4=1172. The old Python route omits the amax pass in its read
counter, so encoded-byte equality alone does not qualify native dispatch. Preserve these two
measurement rows beside the returned artifact/one-launch result in the receipt.

Installed real run **17591** passed2026-10-25 on RTX3060 ponyo: Runtimef55db84af/wheel613843dc…,
TensorFS6431a5387/wheel0171ff04…/native850d976d…, TensorD9998cf5f4. Two declared/adopted
outputs verified exact producer bytes and stored integrity, then31actual GPU increments ran
from one host graph launch. Observed logs establish native dispatch: int8read8224/new2096 and
NVFP4read16416/new1172, one newlyencoded tensor each. `native_quantized_tensors` was not retained
in run.log/run.show, so no observed-metric claim is made; the distinct two-pass source counter
and native stage logs supply the route evidence. [Receipt](evidence/f55db84af-rtx3060.json).
An ordinary package recipe via `cozy model quantize` and consuming-model proof remain separate.

## Native adapter and mixed per-component route

The additional CPU endpoint exercises the Runtime adapters after the measured TensorFS0.15.6
primitive passed its independent gates. It runs through the ordinary default-home CLI without a
GPU allocation:

```sh
cozy run /absolute/canonical/worktree/release-gate/suites/execution/native-quantization-caller/adapters \
  seed=156101 --on OWNED_MACHINE --json
```

It checks ten historical empty-array results, two strided nonempty inputs and two genuine native
input refusals (the original TensorFS DecodeError cause must survive the Runtime translation).
The golden encoder lives inside this package's frozen `oracle.py`; current production functions
are the subject, never the reference. Empty carriers are Runtime compatibility behavior, not a
claim that raw native primitives accept zero geometry.

A real checkpoint then feeds a mixed ConvRot/FP8/NVFP4 plan across two components. FP8 makes the
whole-selection native Writer route ineligible; explicit public `quantize_component_into` calls
exercise the adapters and existing Output lifetime. Repeating the first component must reuse
its entries, consume no additional grant and report unknown (`None`) reused quality. Actual
adoption consumes exactly2,660bytes including all companions and inherits both plain biases.
ConvRot/NVFP4 parts are byte-exact against the frozen reference; FP8 is independently checked
against its stored value law. Part roles, dtype and physical geometry must all match.

Two real declared outputs exercise refusals: a zero quality bound publishes no tensor, and a
2,659byte grant permits the first component but refuses the second before publication/adoption.
No cancellation callback, backend or failure is injected. This verifies same-Writer component
reuse; cross-attempt recovery and cancellation remain the separate Writer gates. The legacy
per-component source-byte counter omits NVFP4's maximum scan, so its recorded value is explicitly
not a disk/transfer-volume measurement.

CPU adapter run **17785 passed** after normal local SDK update17780: Runtime4429e98fe/
wheel3271557c and TensorFS e521cc779/wheelca15dd65, TensorD4178. It verified all compatibility,
native-refusal, mixed-output, reuse, byte-budget and quality assertions above. The adopted output
wrote exactly2,660bytes; repeated component quality remained unknown with no new reads/writes.
[Exact compact proof](evidence/4429e98fe-adapters-cpu.json). This is distinct from the public GPU
caller, SVDQ calibration/consumer and encoded LoadSlot0.15.7 qualifications.
