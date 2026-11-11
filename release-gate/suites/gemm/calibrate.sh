# calibrated <drive dir>: the host before any number of it is kept (xs-177). The Runtime's bench measures the pod's
# sustained bf16 GEMM TFLOPS and memory GB/s (`host_calibrate`); either below FLOOR of its card's reference
# (calibration.tsv) refuses the host: it is ended and another rented (gate.sh `again`), as a host too slow to download
# is. An H100 that ran every kernel ~3x slow (release gate 43061813774) is what it stops. A card with no reference
# passes and says so. Any suite can source this and call it first.
calibrated() {
  local floor=0.7 detail card tflops gbps ref why
  ONLY=${ONLY:+$ONLY,host-calibrate,} cell host-calibrate "$1/host_calibrate" --gpus=1
  detail=$(run_get "$OUT" host-calibrate .result.detail)
  card=$(jq -r '.inventory.card // empty' <<< "$detail" 2>/dev/null)
  tflops=$(jq -r '.tflops // empty' <<< "$detail" 2>/dev/null) gbps=$(jq -r '.gbps // empty' <<< "$detail" 2>/dev/null)
  if [ -z "$tflops" ]; then expect host-calibration FAIL "no calibration: $(run_get "$OUT" host-calibrate .reason.message)"; return; fi
  ref=$(awk -F'\t' -v c="$card" '$1 == c {print $2 "\t" $3}' "$HERE/suites/gemm/calibration.tsv")
  if [ -z "$ref" ]; then expect host-calibration PASS "$card: $tflops TFLOPS, $gbps GB/s (no reference yet: add it to calibration.tsv)"; return; fi
  why=$(awk -F'\t' -v t="$tflops" -v g="$gbps" -v f="$floor" -v c="$card" '{ if (t < f * $1 || g < f * $2)
    printf "host off its card: %s ran %.0f TFLOPS and %.0f GB/s against %.0f and %.0f (floor %.0f%%)", c, t, g, $1, $2, f * 100 }' <<< "$ref")
  if [ -z "$why" ]; then expect host-calibration PASS "$card: $tflops TFLOPS, $gbps GB/s against $(tr '\t' ' ' <<< "$ref")"; return; fi
  echo "$why" > host-slow
  expect host-speed FAIL "$why"
  printf 'FAIL %s\nVERDICT FAIL (host off its card)\n' "$why" | tee verdict.txt
  exit 1
}
