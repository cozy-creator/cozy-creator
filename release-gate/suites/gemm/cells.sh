# Suite `gemm` (README.md, xs-177): every GEMM candidate of each lane at our packages' shapes beside the rule's pick, and
# nvfp4's Triton tiles that keep every bit. The measuring is the Runtime's own (`cozy_runtime.bench`); drive/ calls it.
GD=$HERE/suites/gemm/drive
cell gemm-speed "$GD/gemm_speed" seed:=$((R + 770)) --gpus=1
cell gemm-tiles "$GD/gemm_tiles" seed:=$((R + 771)) --gpus=1
