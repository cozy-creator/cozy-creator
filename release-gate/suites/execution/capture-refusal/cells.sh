# A plain step loop whose step reads the host or synchronizes the device: every step runs, values exact; a warm call
# first. Escaped in Runtime 0.35.8 (cr-227) as a loop capture's refusal that stayed open, so the eager rest failed
# `device_fault` (SDXL's first warm call after a cold one on an RTX 5090); Runtime 0.43 has no loop capture.
# Sourced by the runtime suite, or --cells.
i=0
for v in warm item sync; do
  cell "capture-refusal-$v" "$HERE/suites/execution/capture-refusal/gpu" variant="$v" seed:=$((R + 351 + i)) --gpus=1
  i=$((i + 1))
done
