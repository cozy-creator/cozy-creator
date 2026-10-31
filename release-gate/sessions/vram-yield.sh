# vram-yield: a game started on a consumer card gets its memory from cozy within 2 s, mid-job and with models idle,
# and the job is never ended for it (design/vram-yield.md). Escaped on 0.14.3 / Runtime 0.36.7 (xs-088, rtx-3070):
# in a container TensorD saw no foreign use once an executor ran, and its last rung cancelled the running call. One
# rental (an 8 GB card), the session's cozy as TensorD, the machine in shared mode (a desktop's default). The game is
# fixtures/game.py: raw driver allocations from a process that is not cozy's. Cells:
#   idle    SDXL and Qwen warm and idle; an idle_gb game starts: all of it within 2 s; a Qwen call beside it completes
#   midjob  a Qwen call; a midjob_gb game starts once the GPU is busy and exits 8 s later: all of it within 2 s; the
#           call completes, never cancelled, with its image
# params:
#   runtime, tensorfs  `machine update` versions (default: the pod's own)
#   idle_gb, midjob_gb the games (default 5 and 3, of an 8 GB card)
#   traffic_gb         GB the provider bills: the image, SDXL, Qwen and the fixture's environment (default 45)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/game.py" ] || fail "no fixture checkout"
quote "$(param traffic_gb 45)" 3000
rent

args=(--tensord "$(command -v cozy)")
[ -z "$(param runtime)" ] || args+=(--runtime "$(param runtime)")
[ -z "$(param tensorfs)" ] || args+=(--tensorfs "$(param tensorfs)")
step update machine update "$NAME" "${args[@]}" -y || fail "update"
cd "$FIX" || fail "no fixtures"
value() { cozy run show "$(runno "$1")" --once --json > "$OUT/$1.json" 2>&1; jq -r '.result.value // empty' "$OUT/$1.json"; }
status() { jq -r '.status // empty' "$OUT/$1.json"; }
run() { local n=$1; shift; step "$n" run "$@" --on "$NAME" || fail "$n"; value "$n" > "$OUT/$n.value"; }
step mode run ./game.py --on "$NAME" op=mode cfg='{"mode": "shared"}' || fail "mode"
step restart machine update "$NAME" --gpu-memory-limit off -y || fail "restart"
[ "$(cozy machine show "$NAME" --json | jq -r .status.mode)" = shared ] || fail "the machine is not in shared mode"
judge() {  # judge <cell> <game record file>: all of it within 2 s
  local s r; s=$(jq -r '.alloc_s // 999' "$2"); r=$(jq -r '.reached // false' "$2")
  say "$1 game: $(jq -c '{phase, gb: .cfg.gb, alloc_s, fails, held_mib, at_trigger: .nvml_at_trigger, at_done: .nvml_at_done}' "$2")"
  [ "$r" = true ] && awk -v s="$s" 'BEGIN{exit !(s <= 2.0)}' || fail "$1: the game had its memory after ${s}s (reached $r)"
}

run warm-sdxl sdxl/generate prompt="a red barn in snow, photo" seed:=$RANDOM
run warm-qwen qwen-image-2/generate prompt="a lighthouse at dusk, photo" seed:=$RANDOM megapixels:=1
run idle-game ./game.py op=start cfg="{\"name\": \"idle\", \"gb\": $(param idle_gb 5), \"trigger\": \"now\", \"give_up_s\": 60, \"hold_s\": 900, \"wait_s\": 70}"
judge idle "$OUT/idle-game.value"
run idle-qwen qwen-image-2/generate prompt="a tea house in rain, photo" seed:=$RANDOM megapixels:=1
[ "$(status idle-qwen)" = completed ] || fail "idle: the Qwen call beside the game is $(status idle-qwen)"
run idle-stop ./game.py op=stop cfg='{"name": "idle"}'

run mid-arm ./game.py op=start cfg="{\"name\": \"mid\", \"gb\": $(param midjob_gb 3), \"trigger\": \"util\", \"util_pct\": 80, \"util_s\": 3, \"delay_s\": 1, \"give_up_s\": 60, \"hold_s\": 8}"
run mid-qwen qwen-image-2/generate prompt="a fox reading a newspaper in a cafe, photo" seed:=$RANDOM megapixels:=1
run mid-game ./game.py op=status cfg='{"name": "mid"}'
judge midjob "$OUT/mid-game.value"
[ "$(status mid-qwen)" = completed ] || fail "midjob: the Qwen call is $(status mid-qwen): $(jq -c .reason "$OUT/mid-qwen.json")"
jq -e '.output[] | select(.status == "completed")' "$OUT/mid-qwen.json" > /dev/null || fail "midjob: the Qwen call left no image"
say "PASS: the game had its memory within 2 s idle and mid-job; every call completed, none ended for it"
