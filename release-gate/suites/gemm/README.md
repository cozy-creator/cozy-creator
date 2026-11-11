# GEMM

Would a measured kernel table help GEMMs the way xs-171's helps attention (tracker `cross-cutting/xs-177`)? Inside a lane
the Runtime picks a GEMM by rule: the family's prebuilt kernel and its tile `choose()`, else cuBLASLt, comfy-kitchen or
Triton. This suite times every candidate at our packages' real shapes beside that pick. A measurement: no pass bar.

- `gemm-speed`: per shape (`SHAPES` in the Runtime's `internal/bench/gemm.py`: H3 15 s, Qwen-Image 4 MP, Klein, SDXL),
  every candidate of every lane in one interleaved set, three rounds: bf16 cuBLAS and cuBLASLt; int8 and fp8 prebuilt
  tiles, cuBLASLt and comfy-kitchen; nvfp4 W4A4 and W4A16; w4a8 tiles. `rules` names the rule's pick per lane, and
  `same_bits` holds each tile's output to it.
- `gemm-tiles`: nvfp4's Triton decode and GEMV over tile settings that keep every output bit (the per-card question).
- `h3-split.sh` (`cells=`, one H100): H3 Turbo 15 s pinned to 8 bits and at the defaults, plus the attention bench at
  H3's shapes, so the time between them splits into GEMM, attention and the rest.

The measuring is the Runtime's own (`cozy_runtime.bench`); `drive/` only calls it, as the attention suite's does.

```sh
gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref rc/0.25.0 -f hub=prod -f suites=gemm -f skus=rtx-4090 \
  -f runtime_wheel=<cozy-runtime dev-build run id> -f establish=true -f publish=false
```
