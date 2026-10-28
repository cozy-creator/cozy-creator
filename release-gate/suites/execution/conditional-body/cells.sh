# Each node class inside the native conditional body denoise_loop records (IF -> WHILE -> body), one run per variant,
# so a native fault ends one executor and names its variant. Escaped in Runtime 0.34.2: driver 615 crashed
# instantiating a body holding a memcpy node (Qwen runs 19373, 19401, SDXL 19378, memory-retry); fixed by
# cozy-runtime#1860. Sourced by the runtime suite, or --cells.
for v in scalar memset memset32 memcpy reduce parallel triton triton_pdl cublas cudnn sdpa mix; do
  cell "conditional-body-$v" "$HERE/suites/execution/conditional-body/gpu" variant="$v" seed:=$((R + 331)) --gpus=1
done
