# --cells file for package releases that ship their Triton sets prebuilt (`_triton`, xs-089; packages #749, #753).
# qwen-cold is Qwen-Image-2's first request at 4 MP right after the --pins release installs on a machine whose kernel
# store never held its sets. Its steps run the fused plans from step 1, and qwen-warm (the same request again) gives the
# same bytes. Every other cell named in FUSED is held to the first rule too, since its first call comes cold from install.
# krea is Krea 2's smoke, which products.sh lacks.
QC=$(pkg qwen-image-2 generate) QP="$OBJECT on a marble table, soft window light, photo"
cell qwen-cold "$QC" prompt="$QP" megapixels:=4 aspect_ratio=1:1 seed:=$((R + 30))
cell qwen-warm "$QC" prompt="$QP" megapixels:=4 aspect_ratio=1:1 seed:=$((R + 30))
cell krea "$(pkg krea-2 generate_turbo)" prompt="$SCENE, photo" seed:=$((R + 31))
# fused <cell>: every step track ran one set of plans from its first step, none of them eager.
fused() {
  [ -f "$1.number" ] || return 0
  local p; p=$(jq -c '[.. | objects | select(has("plans")) | .plans]' "$OUT/$1.json" 2>/dev/null)
  expect "$1-fused" "$(jq -r 'if length > 0 and all(.[]; length == 1 and .[0][0] <= 1 and (.[0][1] | test("fused") and (test("eager") | not)))
    then "PASS" else "FAIL" end' <<< "${p:-[]}" 2>/dev/null || echo FAIL)" "plans ${p:-none}"
}
for c in ${FUSED:-qwen-cold}; do fused "$c"; done
if [ -f qwen-cold.number ] && [ -f qwen-warm.number ]; then
  a=$(run_get "$OUT" qwen-cold '.output[]? | select(.name == "image") | .sha256') b=$(run_get "$OUT" qwen-warm '.output[]? | select(.name == "image") | .sha256')
  expect qwen-cold-digest "$([ -n "$a" ] && [ "$a" = "$b" ] && echo PASS || echo FAIL)" "cold ${a:-none}, warm ${b:-none}"
fi
