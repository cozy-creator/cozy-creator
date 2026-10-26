# Matched Cozy / ComfyUI comparison and profiling

The updated assignment prioritizes MiniMax H3 and Qwen Image 2.1 on RTX 5090 or
H100; SDXL 1024×1024, 30 steps remains a reference. The initial window allows one
GPU, at most 60 minutes and $5 including known storage and traffic.
This harness extends the retained Comfy HTTP/websocket driver and ordinary Cozy
CLI release gates. It changes no production inference implementation.

Before renting, freeze the exact artifacts, model repository commits and TensorFS
checkpoint IDs separately, Comfy revision/dependencies, weights and representations,
actual sigma arrays, guidance, geometry, prompts and request sequence. Use fresh
seeds and actual step/interval evidence to exclude completed-output memoization.
Every saved image must be downloaded, reopened and hashed. Keep all samples.

Record provisioning, package/download setup, first inference, explicit warm
preparation, warm requests, executor/stage/device timing and caller through usable
output separately. Retain thermal/clock observations and cache identity. Compare
caller locations and output boundaries; identify format/RNG differences. Report
median/range only from actual samples and profile measured losses before proposing
production changes. Historical pre-rewrite receipts require exact provenance and
remain separate when weights/settings or the current Hub contract cannot match.

The frozen initial integration base is Creator bdc699a95 / Runtime 774624133 /
packages 8c80f71c. It does not qualify the primary's later native-loading/streaming
implementation. Its exact file hashes were verified before preparation. Runtime774
cannot be used for H3 because it lacks an import required by H3 tools. The revised
first-window plan uses Qwen Image 2.1 INT8 at 1024×1024/30 steps on RTX5090, with
one cold and ten warm requests per engine, plus separate warm CPU/CUDA profiles.
H3 needs the corrected Runtime artifact and common AdaLN/PDD operands before a
matched run. No GPU benchmark has been run for this branch yet.

Session receipts and the final report live under
`/home/fidika/cozy_v2/outputs/stock-torch-20261025/4090-comparison/`.
Use the existing independent rental expiry/finally teardown and verify the exact
owned rental ended/absent. Shared CLI/SDK changes require the existing lock and
restoration of the actual observed installation. No merge or release is authorized.
