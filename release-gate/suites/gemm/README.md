# GEMM

Inside a lane the Runtime picks a GEMM by rule: the family's prebuilt kernel and its tile `choose()`, else cuBLASLt,
comfy-kitchen or Triton (tracker `cross-cutting/xs-177`). The rules are fitted once; this suite holds them against every
candidate at our packages' real shapes each Runtime promotion, so a rule that drifted (torch 2.15's cuBLASLt overtook
Hopper's fp8 tiles) fails here.

- `host-calibrate` (`calibrate.sh`): the pod's sustained bf16 GEMM TFLOPS and memory GB/s against its card's reference
  (`calibration.tsv`). Below 70% of either, the host is refused and another rented, before any number of it is kept.
- `gemm-speed-<model>`: per shape (`SHAPES` in the Runtime's `internal/bench/gemm.py`: H3 15 s on 1 and 4 GPUs, Qwen-Image
  4 MP, Klein, SDXL, an LM layer at 1, 16 and 256 rows), every candidate of every lane in one set, a sample of each a
  round: bf16 cuBLAS and cuBLASLt; int8 and fp8 prebuilt tiles, cuBLASLt and comfy-kitchen; nvfp4 W4A4 and W4A16; w4a8
  tiles. `rules` names the rule's pick per lane, `same_bits` holds each tile's output to it.
- `gemm-tiles`: nvfp4's Triton decode and GEMV over tile settings that keep every output bit.
- `check.py`: a rule more than 10% slower than the best candidate, time-weighted over a model's shapes, fails; so does a
  tile whose bits differ. One shape over 10% is a note.
- `h3-split.sh` (`cells=`, one H100): H3 Turbo 15 s pinned to 8 bits and at the defaults, plus the attention bench at
  H3's shapes, to split a gap into GEMM, attention and the rest.

```sh
gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref rc/<x> -f hub=prod -f suites=gemm \
  -f runtime_wheel=<candidate> -f establish=true -f publish=false     # one pod per card in skus
```
