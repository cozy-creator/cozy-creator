# A denoise_loop step no capture records (a host read, a device synchronize): the recording fails with a CUDA capture
# error, the loop refuses it and runs the rest eagerly, values exact; a warm call first, so no kernel is still opening.
# Escaped in Runtime 0.35.8 (cr-227): the failed capture stayed open after its refusal, so the eager rest failed
# `device_fault` (SDXL's first warm call after a cold one on an RTX 5090). Sourced by the runtime suite, or --cells.
i=0
for v in warm item sync; do
  cell "capture-refusal-$v" "$HERE/suites/execution/capture-refusal/gpu" variant="$v" seed:=$((R + 351 + i)) --gpus=1
  i=$((i + 1))
done
