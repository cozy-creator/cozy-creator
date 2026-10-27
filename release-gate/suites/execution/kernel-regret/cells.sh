# Kernel regret on the gate's card: every prebuilt GEMM tile and dense attention candidate against
# the rule's pick at product shapes. A measurement, not a pass bar. Opt-in (`cells=`) on a measurement
# session only: its own kernel opens read as request-path blocking, so the gate reads FAIL.
cell kernel-regret "$HERE/suites/execution/kernel-regret/gpu" seed:=$((R + 321)) --gpus=1
