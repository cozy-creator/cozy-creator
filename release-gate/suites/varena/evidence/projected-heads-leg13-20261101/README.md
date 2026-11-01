# Initial Heads GPU execution and fixture corrections

All runs used the unchanged Runtime candidate `dae973282`, package `3476cb1d`, and
TensorFS `b20f699c` on leopold's RTX4090, UUID `001d94d1-0a24-f46e-9917-aeacf408c95f`.
These are preserved failed fixture attempts, not a completed Heads qualification.

| Run | Executed boundary and finding | Fixture correction |
|---|---|---|
| 25203 | Writable seed was declared as immutable Loop input; the Loop disables inference mode. | Actual carried seed and KEY_MEAN/Census rebinding (`3e470da58`). Real CPU Torch and Runtime `_owned`/`_call` checks reproduce and correct it with and without dispatch instrumentation. |
| 25211 | Whole/28/7/1 native head observations published and minimum refused; after-write readback exhausted capacity before its boundary assignment. | Read every completed contiguous output row directly to CPU, assert zero additional device allocations, and retain explicit boundary stages (`7a529010a`). |
| 25213 | The after-write refusal completed; the first full processor had one native Heads observation and a legacy output projection. | A real normal-input call learns the existing single-consumer projection contract (`91f8f3d6c`); no private ownership flag is assigned. |
| 25215 | Complete 56-head processor recorded 56 seeded heads and two native observations; odd55 whole/28+27/minimum cases ran. Its separate encoded output had illegal K=7040. | Validate every construction with the provider's real eligibility method. Keep odd-head operation coverage; add valid 56-head/513-token full-processor coverage with the unchanged strict two-native assertion. |

The pinned Torch copy implementation explains the second observer issue: a noncontiguous
CPU/GPU copy materializes contiguous source storage. Row-wise same-dtype copies avoid that
extra device buffer; full first-head coverage and the managed cap are retained.
[Torch 08187 CUDA copy implementation](https://raw.githubusercontent.com/pytorch/pytorch/08187d9e0fba026dc8217405802ab5381dc88d90/aten/src/ATen/native/cuda/Copy.cu).

CPU eligibility now confirms Q/K/V are valid for both head geometries. The ConvRot256
provider accepts both 56-head output cases and rejects the 55-head output, before any
invalid encoded leaf is constructed. The revised fixture has 14 cases, including an
explicit output-encoding refusal. It still needs a complete GPU pass and cleanup.

The unchanged standalone projection regression **25204** and same-process cleanup **25207**
pass: six cases, three actual automatic-workspace checks, all numerical errors zero.
The unchanged complete Tail regression **25208** and cleanup **25209** pass: five cases,
14 automatic-workspace checks, all numerical errors zero. Their fixture-owned storage
returns to the exact initial 17,039,360-byte allocated / 20,971,520-byte mapped baseline;
this is not a global-zero claim. Native request keys are gone before cleanup.

No Runtime, package, allocator, kernel, numerical threshold, or accepted-operation fallback
was changed to address these fixture findings. Full-model comparison remains pending.
