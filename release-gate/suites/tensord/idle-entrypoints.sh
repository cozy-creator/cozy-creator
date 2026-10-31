# Two constructions of one package, the second's call while the first's idles with its weights resident (19472/19480:
# idle H3 groups of one package kept their copies while the active one sat at the card limit, stalling 26-96 s per
# segment). Since xs-113 one executor per package environment, lane and degree loads both: the second runs on the
# first's processes, which TensorD neither offloads nor ends; their Runtime evicts the idle construction's blocks when
# it needs the room. Sourced by gate.sh --cells (or sessions/one-executor.sh) on a rental of 1 or `$GPUS` GPUs (the H3
# cells' group); `only` picks the cells (idle-klein-a,-b,-a2,-b2, idle-h3-s,-t,-lf, idle-cancel,-after-cancel,
# idle-h3-cancel,-after-cancel).
# idle_entrypoints.py judges:
#   idle-klein-b, -a2, -b2, idle-h3-t  on the previous cell's processes, none of them offloaded or ended between the two
#                      calls; Klein's with none of its own weights read twice (each fits alone, two never)
#   idle-h3-lf         a two-piece shot's pieces on idle-h3-t's processes: one construction serves both
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
# One character picture, as a project sends it: no model but H3's runs.
ln -sfn "$REF" "idle-ref.${REF##*.}"
lead="assets: [{name: Lead, kind: character, image: ./idle-ref.${REF##*.}}]"
# The pair's shots are 5 s: what is measured is the executor the second runs on, not the clip (the standard
# construction's fewest steps, 30, cost a 15 s clip about an hour on a 4090).
H3=$(pkg minimax-h3 shot)
h3_shot() {  # h3_shot <file> <prompt> <seed> [yaml lines]
  printf '%s\nprompt: "<Lead> %s %s"\nduration_s: 5\nseed: %d\n%s' "$lead" "$MOTION" "$2" "$3" "${4:-}" > "$1"
}
h3_shot idle-h3-standard.yaml "The camera holds, then pans right." $((R + 409)) $'turbo: false\nsteps: 30\n'
h3_shot idle-h3-turbo.yaml "The camera drifts left." $((R + 404))
# Two pieces (16 s: 10 s, then 6 s cut between its beats), both on idle-h3-t's Turbo Ref2VA construction.
h3_shot idle-h3-pieces.yaml "A slow push in." $((R + 405)) \
  $'beats: [{at: 0, prompt: "<Lead> walks toward the camera."}, {at: 8, prompt: "<Lead> turns to the camera."}]\n'
sed -i 's/^duration_s: 5$/duration_s: 16/' idle-h3-pieces.yaml
cell idle-h3-s "$H3" --input idle-h3-standard.yaml --gpus="${GPUS:-1}"
cell idle-h3-t "$H3" --input idle-h3-turbo.yaml --gpus="${GPUS:-1}"
cell idle-h3-lf "$H3" --input idle-h3-pieces.yaml --gpus="${GPUS:-1}"
# The same in a group: the standard construction canceled in its denoise, then Turbo on whatever comes back.
h3_shot idle-h3-cancel.yaml "A crane shot rises over the scene." $((R + 410)) $'turbo: false\nsteps: 30\n'
h3_shot idle-h3-after-cancel.yaml "The camera circles slowly." $((R + 411))
canceled idle-h3-cancel cell idle-h3-cancel "$H3" --input idle-h3-cancel.yaml --gpus="${GPUS:-1}"
cell idle-h3-after-cancel "$H3" --input idle-h3-after-cancel.yaml --gpus="${GPUS:-1}"
"$CLI" machine logs "$M" memory -n 5000 > idle-memory.log 2>&1
cap=$("$CLI" --json machine show "$M" 2>/dev/null | jq '.status.gpus[0].memory_bytes // 0')
python3 -I "$HERE/suites/tensord/idle_entrypoints.py" . idle-memory.log "$cap" > idle-entrypoints.tsv
while IFS=$'\t' read -r name verdict detail; do expect "$name" "$verdict" "$detail"; done < idle-entrypoints.tsv
