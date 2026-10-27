# Kernel regret on the gate's card: every prebuilt GEMM tile and dense attention candidate against
# the rule's pick at product shapes. A measurement, not a pass bar. Use with --cells beside a suite.
cell kernel-regret "$HERE/suites/execution/kernel-regret/gpu" seed:=$((R + 321)) --gpus=1
