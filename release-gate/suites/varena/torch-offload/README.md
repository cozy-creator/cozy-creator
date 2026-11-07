# vt.offload on a real GPU

Proves the promises in [TORCH-OFFLOAD.md](../../../../crates/varena/TORCH-OFFLOAD.md). Each function runs a fresh process, because stock Torch must have
no Varena in it and `vt.install` must come before CUDA.

```sh
cozy run release-gate/suites/varena/torch-offload/adapter --on MACHINE --gpus 1
cozy run release-gate/suites/varena/torch-offload/in_executor --on MACHINE --gpus 1
cozy run release-gate/suites/varena/torch-offload/sdxl --on MACHINE --gpus 1 --input request.yaml
```

`adapter` runs tiny real models: a dropped handle, ties and identities, direct reads and aliases, an alias that escapes a block, the VAE's dtype flips,
writes, streams, a block too big to pin, a loop's graph and a fork. `in_executor` runs inside the Runtime's own process, on its allocator and stream.

`sdxl` makes two full calls of unmodified Diffusers. `mode` is `stock`, `stock-cpu-offload`, `eager` or `graphs`. A capped run sets `limit_bytes` to the
machine's `--gpu-memory-limit`, which a fresh process doesn't inherit. It reports total, step and first-step times, NVML peak, `weights_filled` and host
memory, and saves the images: look at them. Give every run a new prompt and seed.
