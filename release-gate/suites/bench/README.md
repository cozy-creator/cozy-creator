# Suite `bench`: the fixed GPU benchmark

Every release candidate runs the same cells on the same GPU types, and each is held to the numbers the last release proved
and to ComfyUI's. A cell passes only when its output is right as well as fast.

```
release-gate/bench.sh --runtime F --tensorfs F --tensord F [--packages DIR] [--groups rtx-4090x1]
release-gate/bench.py table <gate dir>...           # again, from the gate dirs
release-gate/bench.py baseline <version> <gate dir>... [--comfyui suites/bench/comfyui.json]  # after a release
```

## What runs

- `skus`: the groups (`<sku>x<gpus>`), one fresh pod each, held to a fixed host RAM (30.46 GiB a GPU) by `pod.sh` over SSH.
- `cells.tsv`: the cells and what each guards. Each runs once cold on its fixed seed (the judged output) and then `warm`
  times on fresh seeds (the judged times). Weights download before the cold run (gate.sh `ahead`).
- `calibrate/`: a reference workload and the pod's facts, before and after the cells: fixed GEMM, attention and convolution,
  the host-to-device copy, a cold disk read, and the host CPU (single-thread Python, 10k tiny kernel launches, a CLIP-style
  tokenize, a fork, a fresh `import torch`, and its executor's own fork and CUDA init; CPU model, cores, quota). Eager steps
  are host-bound: a 2-4x slower host CPU (euclase, 11-10) stepped SDXL at 193 ms against 120 on a normal GPU. A pod more
  than 12% off its GPU type's stored kernels (35% copy, 60% disk, 50% CPU probes, 75% fork), or over the CPU ceilings
  (executor fork 220 ms, CUDA init 1.3 s), is ended and replaced before any cell; in a table it fails the group. More than
  3% kernel drift over the run is noted, as is a cell whose executor started 1.5x slower than its baseline's.

## ComfyUI

`bench.sh --comfyui` runs ComfyUI after the cells on the same pod, under the same host RAM, for the cells whose `comfyui`
column names a cell of the benchmarks runner (`comfyui.sh`, `comfy.py`; cozy-creator/benchmarks pinned there), with the
cells' own prompts and seeds. Its rows (`comfyui-rows.json`) are judged beside the cells and go into the next baseline;
without the flag the baseline's stored rows are used.

## Verdict (`bench.py judge`)

A cell FAILs when
- a run failed, did not run, or the memo answered it;
- its output is no output (flat, a structure jump between frames, wrong size, frames or sound), or differs from the
  stored output of the same request and seed (pictures: layout SSIM < 0.90; video: any eighth > 0.08 mean grey difference,
  or late eighths > 2.5x the first; sound envelope < 0.6), or a multi-GPU output from its one-GPU twin (`like`);
- its warm steady step or total is over the baseline by more than max(5%, 3 sd of the two medians' ratio), uncapped cells
  scaled by each call's SM clock first (a total by its denoise part);
- its peak VRAM is over its cap, its multi-GPU run does not beat the one-GPU cell (`beats`) or takes another transport;
- it loses to ComfyUI where the baseline won.

`known-issues.json` (`cell`, `group`, `issue`, `reason`) makes a FAIL a KNOWN.

## Stored numbers

`../../baselines/<version>.json`: per group the pod, its reference workload, each cell's medians and spread, and ComfyUI's
rows. Reference outputs are assets of the `release-gate-baselines` release (`bench-<version>-<group>-<cell>.<ext>`), fetched
into `baselines/refs/` (untracked).
