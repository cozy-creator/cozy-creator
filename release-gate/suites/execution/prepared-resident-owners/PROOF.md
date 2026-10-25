# Real CUDA proof — 2026-10-25

Ordinary admitted CLI calls on ponyo, NVIDIA GeForce RTX 3060, Torch 2.14.1+cu130.
All eight calls completed with `passed=true` and CLI exit 0. No mocks or unit tests.

## Frozen cohort

| Artifact | Source commit | Wheel/binary SHA256 |
|---|---|---|
|runtime|`de3dfc42c3467bb9df50b660ad613e64a43a12c6`|`ae349ceab666f7ae39c7f204531062be165661cf224587fd7885d9d4b04d840b`|
|tensorfs|`6431a5387dab3125deb77c38f9c48410538ec8a8`|`0171ff047875837509bdc596fa6d1503b35fd205d2fa2d7ca38c7a227a8794ff`|
|tensord|`9998cf5f4ea58f72bbf70efd22e483f235517895`|`f9359e2f00c7f0939a43a79fdb319f6ce504546add808c59a7f44af1e81bddfe`|

Owners/attention fixture: `9104f6d9804ee8dba09831bdfb19d2ca96fc7744`.
SDXL package: `af21cc612a85d69317603dc60881d1bb46e76e2b`; the unchanged copied function
has SHA256 `83d9ca699eb97e6f95fafc785d2607c5db6865eb7187415acf78632b3a191a06`.

## Outcomes

| Run | Cell | Proven result |
|---|---|---|
|17545|resident owners|Zero-read cold refusal and resident capture; one native launch; two component phases; actual LoRA/hook/state/epoch refusals; real OOM restores state and leaves CUDA healthy.|
|17546|attention|One native zero-dropout SDPA launch matches eager output; actual nonzero dropout and random generation refuse without changing state.|
|17547|SDXL component, 2 steps, CFG 1.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|
|17548|SDXL component, 2 steps, CFG 7.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|
|17549|SDXL component, 7 steps, CFG 1.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|
|17550|SDXL component, 7 steps, CFG 7.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|
|17551|SDXL component, 30 steps, CFG 1.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|
|17552|SDXL component, 30 steps, CFG 7.0|Maximum absolute error 0; nonresident capture and unsupported prediction mode refuse.|

The SDXL cases execute the actual package preparation function with a small, real Diffusers
UNet (sample size 8, channels 32/64) and genuine TensorFS weights through Backend/Placement.
The oracle is the same UNet with ordinary Diffusers Euler scheduling. These are component
correctness receipts, not full SDXL checkpoint/image generation, performance, ComfyUI comparison
or a platform release. Full 30-step baseline/candidate images, baked/per-step LoRA and 8 GiB
capacity/streaming qualification remain separate gates.

## Overlap observation boundary

Run17545 asserts the actual request is non-overlapping before capture, immediately afterward,
and after the next-component phase; those assertions passed. Its serialized
`overlap_after_capture=true` was read later at function return after subsequent fixtures, so
that old report field does not describe the immediate post-capture value. The raw receipt is
preserved. The fixture now snapshots the immediate value for future reports; it does not
relax either assertion or rewrite the existing receipt.

## Receipts

Original NDJSON records: `outputs/stock-torch-20261024/native-corrected-20261025/`.
Full decoded details and artifact provenance are retained in the owned worktree at
`.task/GPU-17545-17552.json`. These hashes identify the unchanged original files:

| File | SHA256 |
|---|---|
|`resident-owners.json`|`94f4253215045732bd92a057a89125b2d2ee29620ee6d96b5448340706afc4a5`|
|`zero-dropout-attention.json`|`1c1038b5409a1f4031458210ae69182c901b8c0b8df558fd42eb13ba94fb7dcf`|
|`sdxl-2-cfg1.0.json`|`8e87925c12c2861d024eba00405829370cd41a3a230ee88279ad29948c72d82f`|
|`sdxl-2-cfg7.0.json`|`787cd1809cc1ab1fbbac27dbe675a04457efb799f6b29f1561acd4ee68618124`|
|`sdxl-7-cfg1.0.json`|`2cdcbb80f2bb011d1afac8cc5ecbe86d29a9e8e34d438dbb4bdaec329ad3d43e`|
|`sdxl-7-cfg7.0.json`|`de36079157f41f44dd05d054d5245cdbcf7b26632aef4f38e96e98fa07ca4bdc`|
|`sdxl-30-cfg1.0.json`|`1f10cd6f5a9b2e804e7066326eaf267869d5549edd98332bf0d85a2a229ccb20`|
|`sdxl-30-cfg7.0.json`|`92c989ca5415ca4b8386ac377cc949e2a78ae94d66b27fdc6aef17e7ddc54f34`|
