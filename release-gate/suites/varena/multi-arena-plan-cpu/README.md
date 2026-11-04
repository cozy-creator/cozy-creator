# Native lifetime identity and device-inventory host proof

`prove.py --source REPO --target EXISTING_CARGO_TARGET --out NEW_OUTPUT_DIRECTORY`
compiles an output-only exact copy of the native source with the visibility-local
release-gate harness appended. Product source contains no unit-test module.

The executable uses actual native atomics, mutexes, threads and a private-VM fork.
It covers 8,192 concurrent never-reused identities, exhaustion without wrap,
inactive-plan admission invalidation, 512 concurrent device inventory transitions,
device isolation, ignored-retirement and intent-overflow latches, and inherited
mutex refusal. No CUDA/NVML library may be mapped. It fabricates no Arena/Region or
GPU driver. It qualifies only host metadata behavior, not actual publication,
retirement, shared-library packaging, or the staged GPU helper.

The actual GPU cases are the cache-pages controller's `multi_arena_plan` (one
GPU) and `multi_arena_plan_cross_device` (two actual leased GPUs, payload
`expected_devices=2`). They require the exact matching core/libstd helper bundle;
staging source alone does not make the helper portable-ready. The coordinator owns
build packaging, ordinary Cozy execution, GPU budget and cleanup.
