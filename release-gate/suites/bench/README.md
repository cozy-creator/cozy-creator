# Suite `bench`: the fixed GPU benchmark

Every release candidate runs the same cells on the same GPU types, and each is held to the numbers the last release proved
and to ComfyUI's. A cell passes only when its output is right as well as fast.

```
release-gate/bench.sh --comfyui --previous --runtime F --tensorfs F --tensord F --packages DIR [--groups rtx-4090x1] [--out DIR]
release-gate/bench.py table <gate dir>... [--comfyui FILE]  # again, from the gate dirs
release-gate/bench.py versus <gate dir>... [--previous DIR] # Cozy against ComfyUI on the same pods, H3 by stage
release-gate/bench.py baseline <version> <gate dir>... --comfyui-out FILE   # after a release
```

## What runs

- `skus`: the groups (`<sku>x<gpus>[.<part>]`: a part splits one SKU's cells over two pods), one fresh pod each, held to a
  fixed host RAM (30.46 GiB a GPU) by `pod.sh` over SSH, each with its disk and hard stop, ComfyUI's included.
- `--previous`: each pod first runs the release it boots (the Hub's), then stays up (`gate.sh --keep`) for the candidate
  (`gate.sh --on <pod> --end`): the previous release on the same pod is the candidate's baseline (`previous.json`), and
  ComfyUI runs last. The first passes are staggered by `skus`' estimate to end together; the pods are kept alive until
  all are done; then `--between FILE` runs once, which moves this computer's cozy to the candidate when the candidate is
  another minor (Calcifer sends only to machines on its own minor); then the second passes.
- A capped cell (`cap` GiB) runs on a card of that size, the way ComfyUI does: Cozy's executors leave the GPU (one restart),
  then a separate process (`vram.py`) holds all but `cap` GiB, counting what other processes already hold as theirs. Not
  `--gpu-memory-limit`, which caps Cozy alone. Its peak VRAM is the driver's less the holder's.
- `cells.tsv`: the cells and what each guards. Each runs once cold on its fixed seed (the judged output) and then `warm`
  times on fresh seeds (the judged times). Weights download before the cold run (gate.sh `ahead`).
- `calibrate/`: a reference workload and the pod's facts, before and after the cells: fixed GEMM, attention and convolution,
  the host-to-device copy, a cold disk read, and the host CPU (single-thread Python, 10k tiny kernel launches, a CLIP-style
  tokenize, and its executor's own fork and CUDA init; CPU model, cores, quota), all within the package contract; the GPU's
  clock and temperature from TensorD's reads of that call, its name, driver, PCIe link and power limit from `pod.sh gpu`. Eager steps
  are host-bound: a 2-4x slower host CPU (euclase, 11-10) stepped SDXL at 193 ms against 120 on a normal GPU. A pod more
  than 12% off its GPU type's stored kernels (35% copy, 60% disk, 50% CPU probes, 75% fork), or over the CPU ceilings
  (executor fork 220 ms, CUDA init 1.3 s and 0.4 s more a GPU), is ended and replaced before any cell, twice; the third host
  runs anyway, labelled; in a table it fails the group. More than
  3% kernel drift over the run is noted, as is a cell whose executor started 1.5x slower than its baseline's.

## ComfyUI

`bench.sh --comfyui` runs ComfyUI after the cells on the same pod, under the same host RAM, for the cells whose `comfyui`
column names a cell of the benchmarks runner (`comfyui.sh`, `comfy.py`; cozy-creator/benchmarks pinned there): the same
model, shape, steps, prompts and seeds, ComfyUI set to the kernels Cozy served. A matched cell's warm runs each get their
own prompt (`vary`), so neither engine reuses a text encoding. A capped cell's ComfyUI runs beside the same holder. Its
rows (`comfyui-rows.json`) carry warm total, steady step, peak VRAM and cold; `bench.py versus` (`comfyui.md`) puts them
beside Cozy's and names every cell Cozy loses. Peak VRAM on a whole card is Cozy's executor's own peak plus its CUDA context
(the calibrate call's, on that pod) against ComfyUI's NVML peak with ComfyUI alone on the GPU: the driver's figure counts
every executor's resident weights, and varena fills a free card as a cache. ComfyUI's rows stay off the repo until every
cell is won (Paul 11-10): `baseline` writes them to `--comfyui-out`, and `table --comfyui` reads them back.

## Verdict (`bench.py judge`)

A cell FAILs when
- a run failed, did not run, or the memo answered it;
- its output is no output (flat, a structure jump between frames, wrong size, frames or sound), or differs from the
  stored output of the same request and seed (pictures: layout SSIM < 0.90; video: any eighth > 0.15 mean grey difference,
  or an eighth above 0.05 over 2x the one before it, a break partway; kernel paths drift smoothly); sound envelope < 0.6), or a multi-GPU output from its one-GPU twin (`like`);
- its warm steady step or total is over the baseline by more than max(5%, 3 sd of the two medians' ratio), uncapped cells
  scaled by each call's SM clock first (a total by its denoise part);
- its peak VRAM (less the holder's) is over its cap, its multi-GPU run does not beat the one-GPU cell (`beats`) or takes another transport;
- it loses to ComfyUI where the baseline won.

`known-issues.json` (`cell`, `group`, `issue`, `reason`) makes a FAIL a KNOWN.

## Stored numbers

`../../baselines/<version>.json`: per group the pod, its reference workload, each cell's medians and spread (Cozy only). Reference outputs are assets of the `release-gate-baselines` release (`bench-<version>-<group>-<cell>.<ext>`), fetched
into `baselines/refs/` (untracked).
