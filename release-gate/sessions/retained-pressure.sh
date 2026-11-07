# retained-pressure (11-06, run 41362): a loop's retained state never fails a call late. Qwen-Image 2.1 Turbo's two-
# reference edit keeps its prefix K/V (about 4 GB at 2 MP) on the card when the room it sees holds it; a foreign program
# then takes most of the card after step 2. The call must complete, decode included, with no failed working
# allocation: what is kept on the card spills when the core asks, and dies with the loop. One rental (a 24 GB card by
# default), the machine shared (a desktop's default), capped so the game's context finds room. The game is
# fixtures/game.py, raw driver allocations from a process that is not cozy's, started by a root shell over
# `cozy machine ssh` once the call's step 2 is done.
# params:
#   tensord, runtime, tensorfs  the candidate (default: the session's cozy, the pod's own Runtime and TensorFS)
#   packages     a packages_refs name: qwen-image-2 from that checkout ("": the Hub's release)
#   cap          `--gpu-memory-limit` bytes (default 20 GiB of a 24 GB card)
#   game_gb      what the game takes after step 2 (default 18)
#   traffic_gb   GB the provider bills: the image, Qwen-Image 2.1 Turbo and the fixture's environment (default 45)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/game.py" ] || fail "no fixture checkout"
PK=$(param packages '')
T=qwen-image-2/generate_turbo
[ -z "$PK" ] || { [ -d "$PACKAGES/$PK/qwen-image-2" ] || fail "no packages checkout $PK"; T=$PACKAGES/$PK/qwen-image-2/generate_turbo; }
quote "$(param traffic_gb 45)" 2400
rent

args=(--tensord "$(param tensord "$(command -v cozy)")")
[ -z "$(param runtime)" ] || args+=(--runtime "$(param runtime)")
[ -z "$(param tensorfs)" ] || args+=(--tensorfs "$(param tensorfs)")
step update machine update "$NAME" "${args[@]}" -y || fail "update"
step restart0 machine update "$NAME" --gpu-memory-limit off -y || fail "restart"  # writes TensorD's config.toml
step mode run "$FIX/game.py" --on "$NAME" op=mode cfg='{"mode": "shared"}' || fail "mode"
step restart machine update "$NAME" --gpu-memory-limit "$(param cap $((20 << 30)))" -y || fail "restart"
[ "$(cozy machine show "$NAME" --json | jq -r .status.mode)" = shared ] || fail "the machine is not in shared mode"
show() { cozy run show "$(runno "$1")" --once --json --full > "$OUT/$1.json" 2>&1; }

# Its two pictures, made here: a lighthouse on rocks and a teapot, 1 MP each (and the model warm).
for p in photo:"a white lighthouse on dark sea rocks under a grey sky, photo" teapot:"a glazed blue teapot on a white table, photo"; do
  step "${p%%:*}" run "$T" --on "$NAME" --lane int8 prompt="${p#*:}" megapixels:=1 aspect_ratio=1:1 seed:=$RANDOM || fail "${p%%:*}"
  show "${p%%:*}"; [ "$(jq -r .status "$OUT/${p%%:*}.json")" = completed ] || fail "${p%%:*}: $(jq -r .status "$OUT/${p%%:*}.json")"
done
edit() {  # edit <name>: the two-reference edit at 2 MP, its own text and seed
  jq -n --arg a "$(jq -r '.output[0].path' "$OUT/photo.json")" --arg b "$(jq -r '.output[0].path' "$OUT/teapot.json")" --arg w "$1" --argjson s "$RANDOM" \
    '{aspect_ratio: "1:1", megapixels: 2, seed: $s, pictures: [{name: "photo", image: $a}, {name: "teapot", image: $b}],
      prompt: ("Place a " + $w + " copy of the teapot from <teapot> on the rocks below the lighthouse in <photo>, keeping the lighthouse where it is.")}' \
    > "$OUT/$1.req.json"
}
edit warm; step warm run "$T" --on "$NAME" --lane int8 --input "$OUT/warm.req.json" || fail "warm edit"

# The pressure call: the game's launcher is written first, so its start is one ssh once step 2 is done.
step game-write run "$FIX/game.py" --on "$NAME" op=write \
  cfg="{\"name\": \"pressure\", \"gb\": $(param game_gb 18), \"trigger\": \"now\", \"give_up_s\": 90, \"hold_s\": 120, \"wait_s\": 0}" || fail "game write"
cozy run show "$(runno game-write)" --once --json > "$OUT/game-write.json" 2>&1
# A function's value is its JSON text.
value=$(jq -c '.result.value | if type == "string" then fromjson else . end' "$OUT/game-write.json" 2> /dev/null)
launch=$(jq -r '.launch // empty' <<< "$value" 2> /dev/null) st=$(jq -r '.state // empty' <<< "$value" 2> /dev/null)
[ -n "$launch" ] && [ -n "$st" ] || fail "no launcher: $(head -c 300 "$OUT/game-write.json")"
edit pressure
cozy --json run "$T" --on "$NAME" --lane int8 --input "$OUT/pressure.req.json" -p "$RUN_PROJECT" > "$OUT/pressure.ndjson" 2> "$OUT/pressure.err" &
RUNP=$! started=''
while kill -0 "$RUNP" 2> /dev/null; do
  if [ -z "$started" ] && jq -e 'select(.progress.stage == "generating image" and (.progress.completed // 0) >= 2)' "$OUT/pressure.ndjson" > /dev/null 2>&1; then
    cozy machine ssh "$NAME" "setsid nohup sh $launch > /dev/null 2>&1 < /dev/null &" || fail "the game did not start"
    started=$(date -u +%T); say "step 2 done: the game takes $(param game_gb 18) GiB"
  fi
  sleep 0.2
done
N=$(jq -r 'select(.run.number) | .run.number' "$OUT/pressure.ndjson" | head -1)
cozy run show "$N" --once --json --full > "$OUT/pressure.json" 2>&1
cozy machine ssh "$NAME" "cat $st" > "$OUT/game.json" 2>&1
say "game: $(jq -c '{phase, alloc_s, reached, fails, held_mib, at_trigger: .nvml_at_trigger}' "$OUT/game.json" 2> /dev/null)"
say "run $N: $(jq -c '{status, ms: .execution_ms, ooms: .measurements.memory.stats.varena[0].stats.working_allocator.ooms,
  reason: .reason.message}' "$OUT/pressure.json")"
[ -n "$started" ] || fail "the call never reported step 2"
[ "$(jq -r .status "$OUT/pressure.json")" = completed ] || fail "the call failed under pressure: $(jq -r '.reason.message // empty' "$OUT/pressure.json")"
[ "$(jq -r '.measurements.memory.stats.varena[0].stats.working_allocator.ooms' "$OUT/pressure.json")" = 0 ] || fail "working allocations failed under pressure"
jq -e '.reached == true' "$OUT/game.json" > /dev/null || fail "the game never had its $(param game_gb 18) GiB: nothing gave way"
cozy machine ssh "$NAME" "touch ${st%.json}.stop" > /dev/null 2>&1
say "the call completed with the game holding its memory, and no working allocation failed"
