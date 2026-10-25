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
This fixture changes no SDK or rental and is **not yet run**. Native Writer basic/cancel/cancel-write
are separate required gates. An ordinary authored `cozy model quantize` recipe and consuming-model
proof remain required before production caller cutover. No performance claim.
