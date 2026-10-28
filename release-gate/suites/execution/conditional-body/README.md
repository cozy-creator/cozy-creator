# Conditional body

Runtime's native `ResidentRepeat` composes a recorded step into an IF -> WHILE conditional graph. Each
cell records one step per node class through `author.denoise_loop` (no components: tensor work only),
runs its first step eagerly and 3 more natively, and checks the values and that the loop ran resident:

`scalar`, `memset` (8-bit), `memset32`, `memcpy`, `reduce`, `parallel` (a forked stream), `triton`,
`triton_pdl`, `cublas`, `cudnn`, `sdpa`, and `mix` (memset, memcpy, cuBLAS, cuDNN, SDPA and Triton in
one body).

One run per variant (`conditional-body-<variant>`): a crash ends that run's executor only, and the gate
names the variant. A refusal (the loop ran eagerly) fails the cell too.

Escaped in Runtime 0.34.2: on driver 615 (615.71.09 on an RTX 5090, 615.78.08 on an RTX 3060),
`cuGraphInstantiate` read a null pointer when the body held a memcpy node. That crashed native Qwen
(runs 19373, 19401) and SDXL (19378) and the memory-retry cell. cozy-runtime#1860 lowers memset and
memcpy nodes in the body to kernels.

Expected results:

- 0.34.2 on a 615 host: `memcpy` and `mix` fail with signal 11 after `built` (pod-sessions
  40466891536, 40470754858).
- 0.34.2 on other drivers: PASS.
- With cozy-runtime#1860 (Runtime 0.34.4), on every driver: PASS. That includes 615.78.08 on an RTX 3060 (release-gate 40474996907).

The cell catches the bug whenever a gate lands on a 615 host.

```sh
cozy run "$PWD/release-gate/suites/execution/conditional-body/gpu" variant=memcpy seed:=331 --gpus=1 --on MACHINE --json
```

`cells.sh` adds it to the runtime suite.
