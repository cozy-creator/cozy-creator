# Two constructions of one package, the second's call while the first's idles with its weights resident (19472/19480:
# idle H3 groups of one package kept their copies while the active one sat at the card limit, stalling 26-96 s per
# segment). Since xs-113 one executor per package environment, lane and degree loads both: the second runs on the
# first's processes, which TensorD neither offloads nor ends; their Runtime evicts the idle construction's blocks when
# it needs the room. Sourced by gate.sh --cells (or sessions/one-executor.sh) on a rental of 1 or `$GPUS` GPUs (the H3
# cells' group); `only` picks the cells (idle-klein-a,-b,-a2,-b2, idle-h3-s,-t,-lf, idle-cancel,-after-cancel,
# idle-h3-cancel,-after-cancel).
# idle_entrypoints.py judges:
#   idle-klein-b, -a2, -b2, idle-h3-t  on the previous cell's processes, none of them offloaded or ended between the two
#                      calls; Klein's also under the card limit with none of its own weights read twice (each fits alone)
#   idle-h3-lf         long_form's segment on ref2va_turbo's processes: one construction serves both
#   idle-after-cancel, idle-h3-after-cancel  the other construction's call after idle-cancel (idle-h3-cancel) was
#                      canceled mid-call, which ends every construction of its process: completed, on new processes
KA=$(pkg flux-3-klein generate) KB=$(pkg flux-3-klein generate_turbo)
# The bf16 lane (17.6 GiB of weights each): one fits a 24 GiB card, two never fit 32 GiB.
klein() { cell "$1" "$2" prompt="$3" prompt_upsampling:=false megapixels:=1 seed:=$((R + $4)) --lane bf16 --gpus=1; }
klein idle-klein-a "$KA" "$CHAR, studio portrait" 401
klein idle-klein-b "$KB" "$SCENE, photo" 402
klein idle-klein-a2 "$KA" "$OBJECT on a wooden table, product photo" 403
klein idle-klein-b2 "$KB" "$CHAR in $SCENE, film still" 406
# canceled <name> <command...>: the command (one cell), its run canceled once a stage of it generates.
canceled() {
  local name=$1; shift
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* ]] || return 0
  ( for _ in $(seq 1 3600); do
      n=$(cat "$name.number" 2>/dev/null)
      [ -n "$n" ] && [ "$n" != none ] && "$CLI" --json run show "$n" --once 2>/dev/null |
        jq -e '[.. | objects | .stage? | strings] | any(test("generat|denois"))' >/dev/null &&
        { "$CLI" run cancel "$n" -y > "$name.cancel" 2>&1; break; }
      sleep 1
    done ) &
  local watcher=$!
  "$@"
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
}
canceled idle-cancel klein idle-cancel "$KA" "$SCENE at dusk, wide shot" 407
klein idle-after-cancel "$KB" "$OBJECT in $SCENE, macro photo" 408
# One supplied image reference, neither cleaned nor described: no model but H3's runs.
ln -sfn "$REF" "idle-ref.${REF##*.}"
lead="references: [{name: Lead, kind: character, image: ./idle-ref.${REF##*.}}]
clean_references: false
describe_references: false"
# The standard construction at 10 steps: what is measured is the executor it runs on, not its quality.
printf '%s\nprompt: "<Lead> %s The camera holds, then pans right."\nduration_s: 15\nresolution: 768p\nsteps: 10\nseed: %d\n' \
  "$lead" "$MOTION" $((R + 409)) > idle-ref2va.yaml
printf '%s\nprompt: "<Lead> %s The camera drifts left."\nduration_s: 15\nresolution: 768p\nseed: %d\n' \
  "$lead" "$MOTION" $((R + 404)) > idle-ref2va-turbo.yaml
printf '%s\nsegments: [{prompt: "<Lead> %s A slow push in.", duration_s: 15, seed: %d}]\n' \
  "$lead" "$MOTION" $((R + 405)) > idle-long-form.yaml
cell idle-h3-s "$(pkg minimax-h3 ref2va)" --input idle-ref2va.yaml --gpus="${GPUS:-1}"
cell idle-h3-t "$(pkg minimax-h3 ref2va_turbo)" --input idle-ref2va-turbo.yaml --gpus="${GPUS:-1}"
cell idle-h3-lf "$(pkg minimax-h3 long_form)" --input idle-long-form.yaml --gpus="${GPUS:-1}"
# The same in a group: Turbo canceled in its denoise, then the standard construction on whatever comes back.
printf '%s\nprompt: "<Lead> %s A crane shot rises over the scene."\nduration_s: 15\nresolution: 768p\nseed: %d\n' \
  "$lead" "$MOTION" $((R + 410)) > idle-h3-cancel.yaml
printf '%s\nprompt: "<Lead> %s The camera circles slowly."\nduration_s: 15\nresolution: 768p\nsteps: 10\nseed: %d\n' \
  "$lead" "$MOTION" $((R + 411)) > idle-h3-after-cancel.yaml
canceled idle-h3-cancel cell idle-h3-cancel "$(pkg minimax-h3 ref2va_turbo)" --input idle-h3-cancel.yaml --gpus="${GPUS:-1}"
cell idle-h3-after-cancel "$(pkg minimax-h3 ref2va)" --input idle-h3-after-cancel.yaml --gpus="${GPUS:-1}"
"$CLI" machine logs "$M" memory -n 5000 > idle-memory.log 2>&1
cap=$("$CLI" --json machine show "$M" 2>/dev/null | jq '.status.gpus[0].memory_bytes // 0')
python3 -I "$HERE/suites/tensord/idle_entrypoints.py" . idle-memory.log "$cap" > idle-entrypoints.tsv
while IFS=$'\t' read -r name verdict detail; do expect "$name" "$verdict" "$detail"; done < idle-entrypoints.tsv
