# Suite `gemm` (README.md, xs-177): every GEMM candidate of each lane at our packages' shapes beside the rule's pick, and
# nvfp4's Triton tiles that keep every bit; check.py fails a rule that drifted. The measuring is the Runtime's own
# (`cozy_runtime.bench`); drive/ calls it. The host is calibrated first (calibrate.sh). One speed cell per model: a
# child call's result is held to 48 KiB, and one model's shapes stay well inside it.
GD=$HERE/suites/gemm/drive
. "$HERE/suites/gemm/calibrate.sh"
calibrated "$GD"
gemm_n=0
for gemm_model in h3 h3-g4 qwen4mp klein klein-edit sdxl64 sdxl32 lm-m1 lm-m16 lm-m256; do
  cell "gemm-speed-$gemm_model" "$GD/gemm_speed" shapes="$gemm_model" seed:=$((R + 770 + gemm_n++)) --gpus=1
done
cell gemm-tiles "$GD/gemm_tiles" seed:=$((R + 790)) --gpus=1
