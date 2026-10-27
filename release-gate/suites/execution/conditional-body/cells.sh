# Each captured node class inside a native ResidentRepeat conditional body (IF -> WHILE -> body), one child process per
# variant. Escaped in Runtime 0.34.2: driver 615 crashed instantiating a body holding a memcpy node (Qwen runs 19373,
# 19401, SDXL 19378, memory-retry); fixed by cozy-runtime#1860. Sourced by the runtime suite, or --cells.
cell conditional-body "$HERE/suites/execution/conditional-body/gpu" seed:=$((R + 331)) --gpus=1
