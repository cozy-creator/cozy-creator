# --cells file for package releases that ship their Triton sets prebuilt (`_triton`, xs-089; packages #749, #753).
# qwen-cold is Qwen-Image-2's first request at 4 MP right after the --pins release installs on a machine whose kernel
# store never held its sets. Its steps run the fused plans from step 1. qwen-warm is the same request on the release users
# get (unpinned: a pinned rerun is answered from the machine's memo, run 23662), which must give the same bytes, fused and
# computed. Every other cell named in FUSED is held to the first rule too, since its first call comes cold from install.
# krea is Krea 2's smoke, which products.sh lacks.
QC=$(pkg qwen-image-2 generate) QP="$OBJECT on a marble table, soft window light, photo"
cell qwen-cold "$QC" prompt="$QP" megapixels:=4 aspect_ratio=1:1 seed:=$((R + 30))
cell qwen-warm qwen-image-2/generate prompt="$QP" megapixels:=4 aspect_ratio=1:1 seed:=$((R + 30))
cell krea "$(pkg krea-2 generate_turbo)" prompt="$SCENE, photo" seed:=$((R + 31))
# fused <cell>: every step track ran one set of plans from its first step, none of them eager.
fused() {
  [ -f "$1.number" ] || return 0
  local p; p=$(jq -c '[.. | objects | select(has("plans")) | .plans]' "$OUT/$1.json" 2>/dev/null)
  expect "$1-fused" "$(jq -r 'if length > 0 and all(.[]; length == 1 and .[0][0] <= 1 and (.[0][1] | test("fused") and (test("eager") | not)))
    then "PASS" else "FAIL" end' <<< "${p:-[]}" 2>/dev/null || echo FAIL)" "plans ${p:-none}"
}
for c in ${FUSED:-qwen-cold qwen-warm}; do fused "$c"; done
if [ -f qwen-cold.number ] && [ -f qwen-warm.number ]; then
  a=$(run_get "$OUT" qwen-cold '.output[]? | select(.name == "image") | .sha256') b=$(run_get "$OUT" qwen-warm '.output[]? | select(.name == "image") | .sha256')
  x=$(run_get "$OUT" qwen-warm .execution_ms)
  [ -n "$x" ] || expect qwen-warm-computed FAIL "run $(cat qwen-warm.number) has no execution: answered from the memo"
  expect qwen-cold-digest "$([ -n "$a" ] && [ "$a" = "$b" ] && echo PASS || echo FAIL)" "cold ${a:-none}, warm ${b:-none}"
fi
