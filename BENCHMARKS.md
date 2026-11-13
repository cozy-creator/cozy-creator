# Cozy vs ComfyUI

Released Cozy 0.24.0 against ComfyUI v0.45.0. Both ran on the same rented pod, one engine at a time, the same model,
size, steps, prompts and seeds. Both ran in 8-bit. Measured 2026-11-11, with an H3 recheck on 0.25.0 on 2026-11-12.

**Summary:** Cozy is faster on every image model on the RTX 5090 and RTX 4070. On several GPUs it is 1.3–2.4× faster on
H3 video. It runs H3 on cards where ComfyUI runs out of memory. It loses on Ideogram 4 (2.3× slower, fix in progress).
On a 4090 pod with a very slow host-to-GPU link it also loses on H3 (8%) and Qwen-Image (5%).

## How it was measured

- **Versions:** Cozy 0.24.0 (Runtime 0.45.0, TensorFS 0.24.1, each package's release on tensorhub.com) and ComfyUI
  v0.45.0 (`989d93d0`).
- **Precision:** 8-bit on both. Cozy uses its int8 ConvRot lane (`--lane int8`). ComfyUI uses Comfy-Org's int8 ConvRot
  files where they exist, otherwise fp8. ComfyUI is set to the same attention kernels Cozy served.
- **Warm:** the median of the warm requests. Each warm request has its own prompt and seed, so neither engine reuses a
  text encoding.
- **Cold:** the first request after the engine restarts.
- **Times:** seconds from request to finished output. Cozy's time includes its controller, which ran on the CI runner
  beside the pod (0.2–0.5 s).
- **Pods:** each pod passes a disk check and a calibration (GEMM, attention, host-to-GPU copy, CPU) before any cell.
- **Speed-up:** ComfyUI's time divided by Cozy's. Above 1 means Cozy is faster.

| Model | Setting |
|---|---|
| MiniMax H3 Turbo | 15 s video, 1344×768, 362 frames at 24 fps, stereo audio, 8 steps (PDD-8) |
| SDXL 1.0 | 1024², 20 steps, CFG 7 |
| SDXL 1.0 HiDiffusion | 4096² (16 MP), 30 steps, CFG 7.5 ¹ |
| Anima Base 1.0 | 1536², 30 steps, CFG 4.5 |
| MOSS-Image SFT | 2048² (4 MP), 50 steps, CFG 3 |
| Qwen-Image 2.1 | 2048² (4 MP), 40 steps |
| Krea 2 Turbo | 2048² (4 MP), 8 steps |
| Ideogram 4 | 1024², 48 steps, guidance 7, prompt upsampler off |

¹ ComfyUI has no faithful HiDiffusion port, so it runs `comfyui_jankhidiffusion`'s SDXL ultra preset, the one its users
run. Cozy runs the official `hidiffusion` 0.1.10. ComfyUI has no 8-bit SDXL or Anima file, so it casts them to fp8 at load.

## Warm times, one GPU

Cozy / ComfyUI, seconds. **Bold** is the faster.

| Card | H3 Turbo 15 s | SDXL 1024² | SDXL 4096² | Anima | MOSS | Qwen-Image 4 MP | Krea 2 Turbo |
|---|---|---|---|---|---|---|---|
| RTX 5090 | **258.8** / 265.0 | **1.58** / 2.31 | **31.5** / 56.7 | **10.8** / 14.8 | **59.8** / 60.3 | **29.2** / 30.8 | **13.4** / 15.6 |
| RTX 4090 ² | 386.3 / **357.8** | **2.18** / 2.61 | **44.5** / 98.8 | **14.5** / 22.0 | **79.4** / 80.4 | 42.1 / **40.1** | **19.9** / 22.1 |
| RTX 4070 12 GB | **872.8** / out of GPU memory | **7.79** / 10.98 | **111.4** / 242.0 | **36.6** / 55.1 | **187.4** / 192.0 | **97.6** / 99.8 | **52.6** / 59.4 |

² This 4090 pod's host-to-GPU link measured 3.33 GB/s; healthy 4090s measure 13–27. H3 on healthy 4090s:

| RTX 4090 pod | Cozy | ComfyUI | Speed-up |
|---|---|---|---|
| 2× 4090 pod, one GPU used | **329.6** | 343.7 | 1.04× |
| Host RAM held to 30.46 GiB (also has Ideogram, below) | **373.2** | 465.6 | 1.25× |
| Cozy 0.25.0, host RAM held to 30.46 GiB | **335.1** | 474.7 | 1.42× |

ComfyUI took 465.6 and 474.7 s on the two held pods, against 343.7 s on the other, a bigger drop than Cozy's. The
last two rows probably overstate Cozy's usual lead.

## Per card, with cold times and peak VRAM

Peak VRAM is with no memory cap. Cozy keeps weights on the card while there is room and frees it when something else
needs it, so a higher uncapped peak costs nothing.

### RTX 5090

| Model | Warm: Cozy / ComfyUI | Speed-up | Cold: Cozy / ComfyUI | Peak VRAM GiB: Cozy / ComfyUI |
|---|---|---|---|---|
| H3 Turbo 15 s | 258.8 / 265.0 | 1.02× | 284.9 / 302.8 | 30.3 / 31.8 |
| SDXL 1024² | 1.58 / 2.31 | 1.46× | 12.6 / 17.4 | 9.9 / 7.2 |
| SDXL 4096² | 31.5 / 56.7 | 1.80× | 31.2 / 72.9 | 23.8 / 31.3 |
| Anima | 10.8 / 14.8 | 1.37× | 14.5 / 27.2 | 6.5 / 7.5 |
| MOSS | 59.8 / 60.3 | 1.01× | 71.4 / 78.6 | 22.3 / 15.9 |
| Qwen-Image 4 MP | 29.2 / 30.8 | 1.05× | 52.7 / 51.2 | 24.0 / 20.7 |
| Krea 2 Turbo | 13.4 / 15.6 | 1.16× | 19.9 / 37.8 | 23.7 / 23.1 |

### RTX 4090 (pod with the slow 3.33 GB/s host link)

| Model | Warm: Cozy / ComfyUI | Speed-up | Cold: Cozy / ComfyUI | Peak VRAM GiB: Cozy / ComfyUI |
|---|---|---|---|---|
| H3 Turbo 15 s ⁵ | 386.3 / 357.8 | 0.93× | 412.6 / 356.5 | 22.9 / 23.9 |
| SDXL 1024² | 2.18 / 2.61 | 1.20× | 9.4 / 8.4 | 8.8 / 7.3 |
| SDXL 4096² | 44.5 / 98.8 | 2.22× | 44.7 / 106.2 | 22.3 / 23.9 |
| Anima | 14.5 / 22.0 | 1.51× | 23.9 / 26.2 | 6.5 / 7.6 |
| MOSS | 79.4 / 80.4 | 1.01× | 88.1 / 87.3 | 20.6 / 16.0 |
| Qwen-Image 4 MP | 42.1 / 40.1 | 0.95× | 58.2 / 46.3 | 20.0 / 20.6 |
| Krea 2 Turbo | 19.9 / 22.1 | 1.11× | 30.3 / 28.5 | 19.4 / 21.2 |

⁵ Cold and peak VRAM are from the run with attention pinned to Kitchen, which took 395.6 s warm.

### RTX 4090 (host RAM held to 30.46 GiB)

| Model | Warm: Cozy / ComfyUI | Speed-up | Cold: Cozy / ComfyUI | Peak VRAM GiB: Cozy / ComfyUI |
|---|---|---|---|---|
| Ideogram 4 | 68.9 / 29.7 | **0.43×** | 83.9 / 62.7 | 14.4 / 24.0 |
| MOSS | 77.5 / 81.8 | 1.06× | 91.2 / 105.0 | 21.8 / 15.7 |
| Anima | 14.7 / 22.6 | 1.54× | 23.6 / 38.0 | 6.0 / 7.4 |
| H3 Turbo 15 s | 373.2 / 465.6 | 1.25× | 350.1 / 573.8 | 23.3 / 23.7 |

### RTX 4070 (12 GB)

| Model | Warm: Cozy / ComfyUI | Speed-up | Cold: Cozy / ComfyUI | Peak VRAM GiB: Cozy / ComfyUI |
|---|---|---|---|---|
| H3 Turbo 15 s | 872.8 / out of GPU memory | — | 917.7 / — | 11.2 / — |
| SDXL 1024² | 7.79 / 10.98 | 1.41× | 18.7 / 26.4 | 7.1 / 6.6 |
| SDXL 4096² | 111.4 / 242.0 | 2.17× | 114.9 / 268.4 | 10.9 / 12.0 |
| Anima | 36.6 / 55.1 | 1.50× | 45.6 / 67.2 | 6.0 / 6.8 |
| MOSS | 187.4 / 192.0 | 1.02× | 204.8 / 206.7 | 10.4 / 11.8 |
| Qwen-Image 4 MP | 97.6 / 99.8 | 1.02× | 113.0 / 113.9 | 10.1 / 11.9 |
| Krea 2 Turbo | 52.6 / 59.4 | 1.13× | 63.4 / 70.9 | 9.8 / 12.0 |

ComfyUI ran out of GPU memory on H3 here, including with `--lowvram`.

### Cozy only

ComfyUI has no numbers on these two cards. On the 3090 it ran out of host memory on H3 and took its runner down with it;
on the PRO 6000 the pod stopped answering after the restart that frees the GPU for ComfyUI.

| Card | H3 Turbo 15 s | SDXL 1024² | SDXL 4096² | Anima | MOSS | Qwen-Image 4 MP | Krea 2 Turbo |
|---|---|---|---|---|---|---|---|
| RTX 3090 (32 GB host RAM) | 744.9 | 3.94 | 83.0 | 29.0 | 156.7 | 79.0 | 38.3 |
| RTX PRO 6000 | 217.5 | 1.62 | 23.1 | 8.1 | 52.6 | 26.2 | 9.8 |

On the PRO 6000, an earlier pod measured ComfyUI's H3 at 215.9 s, against Cozy's 209.6 s on that pod: a near-tie.

## Several GPUs: H3 Turbo 15 s

ComfyUI splits one request across GPUs with MultiStream.

| Pod | Cozy: 1 GPU / all GPUs | ComfyUI: 1 GPU / all GPUs | Speed-up, all GPUs | Cozy scaling | ComfyUI scaling |
|---|---|---|---|---|---|
| 4× RTX 5090 | 259.6 / **79.3** | 273.5 / 177.6 | 2.24× | 3.27× | 1.54× |
| 4× H100 SXM ³ | 246.9 / **53.6** | 282.9 / 130.6 | 2.44× | 3.16× ⁴ | 2.17× |
| 2× RTX 4090 | 329.6 / **184.9** | 343.7 / 298.5 | 1.61× | 1.78× | 1.15× |
| 2× RTX PRO 6000 ³ | 209.6 / **121.2** | 215.9 / 152.3 | 1.26× | 1.73× | 1.42× |

³ 0.24.0's release candidate. On the H100 the 4-GPU number is Cozy's default kernels (SageAttention2). Holding Cozy to
the 1-GPU run's 8-bit kernels gives 78.0 s, still 1.67× faster than ComfyUI.

⁴ 8-bit kernels on both runs (246.9 / 78.0).

On 0.25.0, Cozy's H3 takes 190.5 s on two 4090s against 321.0 s on one (1.69×).

## Where Cozy loses

- **Ideogram 4 on the 4090, 2.3× slower** (1.31 s a step against 0.56). The gap is compute, in three parts:
  - A useless attention mask kept PyTorch off its fastest attention kernel: about 0.19 s a step. Fixed in the
    0.25.0 release, ideogram-4 0.9.0 (1.25 → 1.07 s a step).
  - Ideogram's published weights are fp8, and GeForce Ada runs fp8 at half speed: about 0.25 s a step. ComfyUI runs an
    int8 ConvRot conversion of the same weights. Cozy's int8 lane is in progress.
  - Norms, modulation and rotary embeddings run as separate ops. ComfyUI fuses them: about 0.2 s a step. Not started.
- **H3 and Qwen-Image on the slow-link 4090:** 8% and 5% slower. On healthy 4090s Cozy wins H3; Qwen-Image hasn't been
  re-measured on one.

## Not measured

FLUX.2, the edit models and the other video models have no ComfyUI comparison yet.

## Sources

Each pod ran in this repository's `release-gate` workflow (`release-gate/suites/bench`). ComfyUI ran after Cozy on the
same pod, through our benchmark runner. A red status on a run comes from the gate's own checks, such as a loss to
ComfyUI or a harness bug fixed since. It doesn't mean the benchmark failed.

| Pod | Run |
|---|---|
| RTX 5090 | [43014082571](https://github.com/cozy-creator/cozy-creator/actions/runs/43014082571) |
| RTX 4090, slow link | [43013605532](https://github.com/cozy-creator/cozy-creator/actions/runs/43013605532) |
| RTX 4090, host RAM held | [43033772528](https://github.com/cozy-creator/cozy-creator/actions/runs/43033772528) |
| RTX 4070 | [43032341163](https://github.com/cozy-creator/cozy-creator/actions/runs/43032341163) |
| RTX 3090 | [43027199393](https://github.com/cozy-creator/cozy-creator/actions/runs/43027199393) |
| RTX PRO 6000 | [43039533939](https://github.com/cozy-creator/cozy-creator/actions/runs/43039533939) |
| 4× RTX 5090 | [43014798864](https://github.com/cozy-creator/cozy-creator/actions/runs/43014798864) |
| 2× RTX 4090 | [43055972352](https://github.com/cozy-creator/cozy-creator/actions/runs/43055972352) |
| 0.25.0, RTX 4090 | [43211705140](https://github.com/cozy-creator/cozy-creator/actions/runs/43211705140) |
| 0.25.0, 2× RTX 4090 | [43217306537](https://github.com/cozy-creator/cozy-creator/actions/runs/43217306537) |
