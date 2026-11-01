# Public Diffusers SDXL sanitizer consumer

This ordinary Cozy release-gate application verifies the complete public SDXL-base
fp16 model staged on the explicitly owned rental and runs the public
`StableDiffusionXLPipeline` at1024x1024 for two Euler denoising steps, including text
encoding and VAE decode. It launches fresh native-Torch and Varena children under
the already installed Compute Sanitizer2025.4.1.0. The Varena child calls
`tensorfs.varena.torch.install()` before CUDA initialization and pipeline import.

This is general Diffusers allocator adoption and memory safety. It does not qualify
Cozy's native int8 pipeline, model CUDA graphs, quality, or performance. No tiny or
random replacement model, Cozy package internals, alternate daemon, tool download,
CUDA context reset, borrowed cuBLAS/cuDNN handle destruction or leak suppression is
used. Model initialization loads public safetensors and refuses network fallback
inside the instrumented children.

## Frozen public inputs

- Model `stabilityai/stable-diffusion-xl-base-1.0`, revision
  `462165984030d82259a11f4367a4eed129e94a7b`, public/ungated.
- Four complete fp16 safetensors plus fourteen config/tokenizer files:
  **6,941,187,536 bytes** total, all sizes and SHA256 values in `manifest.py`.
- No refiner, LoRA, ONNX, alternate VAE or duplicate monolithic checkpoint.
- Torch2.14.0 from PyPI (runtime2.14.0+cu130, git08187d9e), Diffusers0.41.0,
  Transformers5.17.0, Accelerate1.15.0, huggingface-hub1.33.0,
  safetensors0.8.0, NumPy2.5.3. The TensorFS candidate version/extension/shim hashes
  are required payload fields and checked in both fresh children.

At the reported H3 transfer52.8GB, these model files bring required model data to
59.741187536GB, leaving40.258812464GB within100GB for already-accounted and future
environment/image/output/retry traffic. This is the precise unique model-file
size, not a claim about measured network traffic or remaining provider charges.
Root must retain the rental's actual total-transfer/cost guard.

The included offline-resolved lock has compatible wheels for all68 registry
packages. Summing the largest CPython3.12/Linuxx86_64 wheel for each gives a
conservative3,120,530,379-byte cold dependency bound, before cache reuse. Together
with the model and reported H3 traffic this is62.861717915GB, leaving37.138282085GB
for image/build-backend/output/retry/other transfers. Actual transfer accounting
still belongs to root; these byte sizes are not provider billing observations.

## Root execution after review

No GPU execution, dependency installation or model download was performed while
preparing this fixture. `uv.lock` was resolved offline without downloading packages;
exact direct dependency pins are in pyproject.toml. If ordinary `cozy package lock`
needs to bind the staged project to its Hub/account, run it from this directory
and retain the direct pins and resulting lock. Record installed dependency
provenance. Then, only on the owned rental, supply
its actual GPU UUID and candidate hashes in a JSON payload. Stage the exact manifest files
through machine setup first: executor code is network-free. The fixture rechecks every file
size and hash before starting either child:

```json
{
  "tensorfs_version": "0.17.15+dev.g3f3d83ae",
  "extension_sha256": "542c1faf7adf125d7638666091e89149a23b6a20fcc83e5a58a474ff0163e051",
  "shim_sha256": "09684b61d0b909c39d56c7d129432e906ddb0a57f1b4b916b20bb2c4c8d7716e",
  "gpu_uuid": "GPU-REPLACE-WITH-OWNED-RENTAL-UUID",
  "compute_sanitizer": "/var/lib/cozy/xs138-tools/cuda-sanitizer-13.1.1/compute-sanitizer/compute-sanitizer",
  "model_directory": "/var/lib/cozy/xs138-public-diffusers/model-462165984030d82259a11f4367a4eed129e94a7b",
  "mode": "both",
  "seed": 13801
}
```

```bash
cozy run ./release-gate/suites/varena/diffusers-sanitizer/gpu \
  --on "$OWNED_RENTAL" --gpus=1 --input "$PAYLOAD" --json
```

Never omit `--on` or substitute the controller's GPU UUID. The fixture checks the
visible UUID before reading model files. Model bytes stay under
the supplied task-owned `model_directory` on that rental and are shared by the
two sequential children. Both children use the same exact hashed files. `native`
or `varena` mode can run one arm separately; matching both is needed to attribute
library-retained allocations.

## Verdict and complete reports

The wrapper uses `--tool memcheck --leak-check full --error-exitcode99
--target-processes all`. Every line goes to ordinary Cozy telemetry and a complete
raw log on the rental; the result records log hashes, tool version/hash, child
result, exit code and all error-summary counts. Root must save the full
`cozy run show RUN --once --json` receipt before ending the rental.

Cleanup deletes only owned pipeline/output/generator references, collects Python
garbage, synchronizes, trims idle allocator blocks, synchronizes and trims again,
then calls public `torch.accelerator.empty_host_cache()`. Managed bytes/pools/events
and active/allocated pinned-host bytes must reach zero. The strict overall verdict
also requires process exit0 and sanitizer ERROR SUMMARY0.

Known native-library retention is not normalized away. If both arms retain the
same cuDNN/cuBLAS process-lifetime stacks, preserve and compare the full reports:
managed cleanup may pass while the sanitizer gate remains false. Equal byte/error
counts alone do not prove equal causes. Any invalid access or candidate-only
retention is an independent failure. Report this cell separately from full native
Cozy int8/graph qualification.

Primary references: the [public model](https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/tree/462165984030d82259a11f4367a4eed129e94a7b)
and [pinned public Diffusers pipeline](https://github.com/huggingface/diffusers/blob/v0.41.0/src/diffusers/pipelines/stable_diffusion_xl/pipeline_stable_diffusion_xl.py).
