# byo-model: a user's own model on one rental, as the docs show it. A LoRA downloaded to the pod under a local/ name
# runs by that name; a local/ name the pod lacks is refused with its fix; a whole checkpoint, downloaded under a
# local/ name and named by its provider source, replaces the package's model. params:
#   tensord     the pod's TensorD: `image`, a release, or a dev-build run id (image)
#   sdxl        the SDXL function (sdxl/generate@0.1.4)
#   lora        a LoRA source (civitai://135931)
#   whole       a whole SDXL checkpoint source (hf://RunDiffusion/Juggernaut-XL-v9/Juggernaut-XL_v9_RunDiffusionPhoto_v2.safetensors)
#   traffic_gb  GB the provider bills: the image, SDXL, the LoRA and the checkpoint (25)
. "$(dirname "$0")/lib.sh"
TD=$(param tensord image) SDXL=$(param sdxl sdxl/generate@0.1.4) LORA=$(param lora civitai://135931)
WHOLE=$(param whole hf://RunDiffusion/Juggernaut-XL-v9/Juggernaut-XL_v9_RunDiffusionPhoto_v2.safetensors)
BIN=''
if [[ $TD =~ ^[0-9]{6,}$ ]]; then
  gh run download "$TD" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/td" >/dev/null 2>&1 || fail "no cozy artifact in run $TD"
  BIN=$(ls "$OUT"/td/cozy-*/cozy-linux-x86_64 | head -1); chmod +x "$BIN"; say "TensorD $("$BIN" --version)"
elif [ "$TD" != image ]; then BIN=$TD; fi
quote "$(param traffic_gb 25)" 2400
rent
if [ -n "$BIN" ]; then
  step update machine update "$NAME" --tensord "$BIN" -y || fail "update"
  say "pod TensorD $(cozy machine show "$NAME" --json | jq -r .status.version)"
fi
# check <step> <want: completed|failed> [text the reason must hold]: the step's run judged by its record.
checks=0 failed=0
check() {
  local n; n=$(runno "$1")
  [ -n "$n" ] || { say "CHECK $1: no run"; failed=$((failed + 1)); return; }
  cozy run show "$n" --once --json > "$OUT/$1.json" 2>&1
  local status reason; status=$(jq -r .status "$OUT/$1.json"); reason=$(jq -r '.reason.message // ""' "$OUT/$1.json")
  checks=$((checks + 1))
  if [ "$status" = "$2" ] && { [ -z "${3:-}" ] || grep -qF -- "$3" <<<"$reason"; }; then say "CHECK $1: run $n $status${reason:+: $reason}"
  else say "CHECK FAILED $1: run $n $status, wanted $2${3:+ with \"$3\"}: $reason"; failed=$((failed + 1)); fi
}
step dl-sdxl model download _/sdxl@1.0.0 --on "$NAME" || fail "sdxl weights"
step dl-lora model download "$LORA" local/byo-lora --on "$NAME" || fail "the LoRA's download"
step lora-local run "$SDXL" --on "$NAME" --lora model=local/byo-lora,0.8 prompt="a lighthouse on a cliff at dusk, pixel art" seed:=$RANDOM
check lora-local completed
step lora-absent run "$SDXL" --on "$NAME" --lora model=local/byo-never-downloaded,0.8 prompt="a red bicycle, pixel art" seed:=$RANDOM
check lora-absent failed "this machine holds no local/byo-never-downloaded"
step dl-whole model download "$WHOLE" local/byo-whole --on "$NAME" || fail "the checkpoint's download"
step whole-local run "$SDXL" --on "$NAME" model.model=local/byo-whole prompt="a portrait of an old fisherman, photo" seed:=$RANDOM
check whole-local completed
step whole-source run "$SDXL" --on "$NAME" model.model="$WHOLE" prompt="a mountain lake at sunrise, photo" seed:=$RANDOM
check whole-source completed
[ "$failed" = 0 ] || fail "$failed of $checks checks"
say "PASS: $checks checks"
