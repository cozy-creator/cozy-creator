# Conditional body

Runtime's native `ResidentRepeat` composes a captured step into an IF -> WHILE conditional graph.
This cell captures one step per node class, each in its own child process, through the real
`PreparedRepeat.capture`. It runs the step 3 times natively and checks the values:

`scalar`, `memset` (8-bit), `memset32`, `memcpy`, `reduce`, `parallel` (a forked stream), `triton`,
`triton_pdl`, `cublas`, `cudnn`, `sdpa`, and `mix` (memset, memcpy, cuBLAS, cuDNN, SDPA and Triton in
one body).

A crash ends only its child. The run completes when every variant passes. Otherwise it fails, naming
each failing variant's signal or exit code, the last stage it reached, and the GPU and driver.

Escaped in Runtime 0.34.2: on driver 615 (615.71.09 on an RTX 5090, 615.78.08 on an RTX 3060),
`cuGraphInstantiate` read a null pointer when the body held a memcpy node. That crashed native Qwen
(runs 19373, 19401) and SDXL (19378) and the memory-retry cell. cozy-runtime#1860 lowers memset and
memcpy nodes in the body to kernels.

Expected results:

- 0.34.2 on a 615 host: `memcpy` and `mix` fail with signal 11 after `built` (pod-sessions
  40466891536, 40470754858).
- 0.34.2 on other drivers: PASS.
- With cozy-runtime#1860 (0.34.5), on every driver: PASS.

The cell catches the bug whenever a gate lands on a 615 host.

```sh
cozy run "$PWD/release-gate/suites/execution/conditional-body/gpu" seed:=331 --gpus=1 --on MACHINE --json
```

`cells.sh` adds it to the runtime suite.
