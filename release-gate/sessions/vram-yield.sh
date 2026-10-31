# vram-yield: a game started on a consumer card gets its memory from cozy within 2 s, mid-job and with models idle,
# and the job is never ended for it (design/vram-yield.md). Escaped on 0.14.3 / Runtime 0.36.7 (xs-088, rtx-3070):
# in a container TensorD saw no foreign use once an executor ran, and its last rung cancelled the running call. One
# rental (an 8 GB card), the session's cozy as TensorD, the machine in shared mode (a desktop's default). The game is
# fixtures/game.py: raw driver allocations from a process that is not cozy's. A root shell starts it over
# `cozy machine ssh`: started under cozy (a run), cozy's process tree holds it and it is counted as ours wherever the
# driver names the container's pids (driver 610). Cells:
#   idle    SDXL and Qwen warm and idle; an idle_gb game starts: all of it within 2 s; a Qwen call beside it completes
#   busy    the same game as an SDXL call returns, its executor still at work: waited for while it progresses (its
#           bytes fall or its process works), never ended for it, the game has its memory once the give lands
#   midjob  a Qwen call; a midjob_gb game starts once the GPU is busy and exits 8 s later: all of it within 2 s; the
#           call completes, never cancelled, with its image
#   big     the same with a big_gb game that cannot fit beside the call: it keeps the floor it runs on (a ratchet
#           below it failed it) and completes; the game has what is left
#   shrink  a 4 GB game mid-call settles to 1.5 GB and stays: the next Qwen call's first pass runs in the machine's
#           last target for it, which is all the card less H, the game's bytes, the driver's own and the other
#           executors', and it moves within 10% of the bytes of the same call beside a 1.5 GB holder from before it
#   start   a game starts as an executor opens its CUDA context: the call completes, nothing is ended for it, and the
#           GPU's reading is known again once the game exits (an ambiguous start is never taken as foreign for life)
# params:
#   runtime, tensorfs  `machine update` versions (default: the pod's own)
#   idle_gb, midjob_gb, big_gb  the games (default 5, 3 and 5, of an 8 GB card)
#   traffic_gb         GB the provider bills: the image, SDXL, Qwen and the fixture's environment (default 45)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
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
# game <name> <cfg json>: the stand-in game, started by a root shell; waits up to its wait_s for it to have all of it.
game() {
  local n=$1 cfg=$2 launch st t s
  run "$n-write" ./game.py op=write cfg="$cfg"
  launch=$(jq -r .launch "$OUT/$n-write.value") st=$(jq -r .state "$OUT/$n-write.value")
  cozy machine ssh "$NAME" "setsid nohup sh $launch > /dev/null 2>&1 < /dev/null &" || fail "$n: the game did not start"
  t=$(date +%s)
  while [ $(( $(date +%s) - t )) -lt "$(jq -r '.wait_s // 0' <<<"$cfg")" ]; do
    s=$(cozy machine ssh "$NAME" "cat $st 2>/dev/null")
    [ -n "$s" ] && jq -e '.t_done // empty' <<<"$s" > /dev/null 2>&1 && break
    sleep 1
  done
}
step restart0 machine update "$NAME" --gpu-memory-limit off -y || fail "restart"  # writes TensorD's config.toml
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
game idle-game "{\"name\": \"idle\", \"gb\": $(param idle_gb 5), \"trigger\": \"now\", \"give_up_s\": 60, \"hold_s\": 900, \"wait_s\": 70}"
run idle-status ./game.py op=status cfg='{"name": "idle"}'
judge idle "$OUT/idle-status.value"
run idle-qwen qwen-image-2/generate prompt="a tea house in rain, photo" seed:=$RANDOM megapixels:=1
[ "$(status idle-qwen)" = completed ] || fail "idle: the Qwen call beside the game is $(status idle-qwen)"
run idle-stop ./game.py op=stop cfg='{"name": "idle"}'

# busy: the game presses as an SDXL call returns, while its executor records the call's loop at idle (an idle request
# open): its pages go at once though a recording is open, nothing working is ended, the recording stops or moves
# aside, and the same call twice after replays (reused) with the image bytes of the first
memlines() { cozy machine logs "$NAME" memory -n 0 2>/dev/null | wc -l; }
bseed=$RANDOM bprompt="a copper kettle on a stove, photo"
run busy-sdxl sdxl/generate prompt="$bprompt" seed:=$bseed
m0=$(memlines)
game busy-game "{\"name\": \"busy\", \"gb\": $(param idle_gb 5), \"trigger\": \"now\", \"give_up_s\": 120, \"hold_s\": 900, \"wait_s\": 120}"
run busy-status ./game.py op=status cfg='{"name": "busy"}'
w=$(cozy machine logs "$NAME" memory -n 0 2>/dev/null | tail -n +$((m0 + 1)))
say "busy game: $(jq -c '{phase, alloc_s, fails}' "$OUT/busy-status.value"); gives: $(grep -o '"gave":[0-9]*,"gpu":"[^"]*","ms":[0-9.]*' <<<"$w" | tr '\n' ' '); ends: $(grep -o '"why":"[^"]*"' <<<"$w" | sort | uniq -c | tr '\n' ' ')"
[ "$(jq -r '.reached // false' "$OUT/busy-status.value")" = true ] || fail "busy: the game never had its memory"
! grep -q '"why":"no progress' <<<"$w" || fail "busy: an executor making progress was ended for room"
run busy-stop ./game.py op=stop cfg='{"name": "busy"}'
run busy-again1 sdxl/generate prompt="$bprompt" seed:=$bseed
run busy-again2 sdxl/generate prompt="$bprompt" seed:=$bseed
loops() { jq -c '[.measurements.execution.loops[]? | {mode, reused, native_steps}]' "$OUT/$1.json"; }
sha() { jq -r '.output[0].sha256 // empty' "$OUT/$1.json"; }
say "busy after: first $(loops busy-sdxl) $(sha busy-sdxl); again1 $(loops busy-again1) $(sha busy-again1); again2 $(loops busy-again2) $(sha busy-again2)"
[ "$(status busy-again1)" = completed ] && [ "$(status busy-again2)" = completed ] || fail "busy: SDXL after the game did not complete"
jq -e '[.measurements.execution.loops[]? | .reused] | any' "$OUT/busy-again2.json" > /dev/null || fail "busy: the recording did not come back (no replay two calls after)"
[ "$(sha busy-again2)" = "$(sha busy-sdxl)" ] || fail "busy: the replay's image differs from the first call's"

game mid-arm "{\"name\": \"mid\", \"gb\": $(param midjob_gb 3), \"trigger\": \"util\", \"util_pct\": 80, \"util_s\": 3, \"delay_s\": 1, \"give_up_s\": 60, \"hold_s\": 8}"
run mid-qwen qwen-image-2/generate prompt="a fox reading a newspaper in a cafe, photo" seed:=$RANDOM megapixels:=1
run mid-game ./game.py op=status cfg='{"name": "mid"}'
judge midjob "$OUT/mid-game.value"
[ "$(status mid-qwen)" = completed ] || fail "midjob: the Qwen call is $(status mid-qwen): $(jq -c .reason "$OUT/mid-qwen.json")"
jq -e '.output[] | select(.status == "completed")' "$OUT/mid-qwen.json" > /dev/null || fail "midjob: the Qwen call left no image"

game big-arm "{\"name\": \"big\", \"gb\": $(param big_gb 5), \"trigger\": \"util\", \"util_pct\": 80, \"util_s\": 3, \"delay_s\": 1, \"give_up_s\": 60, \"hold_s\": 20}"
run big-qwen qwen-image-2/generate prompt="a lantern-lit tea house in the snow, photo" seed:=$RANDOM megapixels:=1
sleep 25; run big-game ./game.py op=status cfg='{"name": "big"}'
say "big game: $(jq -c '{phase, gb: .cfg.gb, alloc_s, held_mib}' "$OUT/big-game.value")"  # both do not fit: the call is the bar
[ "$(status big-qwen)" = completed ] || fail "big: the Qwen call is $(status big-qwen): $(jq -c .reason "$OUT/big-qwen.json")"

m0=$(memlines)
game shrink-arm '{"name": "shrink", "gb": 4, "trigger": "util", "util_pct": 80, "util_s": 3, "delay_s": 1, "give_up_s": 60, "shrink_to_gb": 1.5, "shrink_after_s": 8, "hold_s": 900}'
run shrink-job qwen-image-2/generate prompt="a red tram on a rainy street, photo" seed:=$RANDOM megapixels:=1
run shrink-next qwen-image-2/generate prompt="a glass teapot of blooming tea, photo" seed:=$RANDOM megapixels:=1
cozy machine logs "$NAME" memory -n 0 2>/dev/null | tail -n +$((m0 + 1)) > "$OUT/shrink-memory.log"
run shrink-stop ./game.py op=stop cfg='{"name": "shrink"}'
# The reference: the same call beside a program that holds what the game kept, from before it (a smaller card).
game shrink-hold '{"name": "ref", "gb": 1.5, "trigger": "now", "give_up_s": 60, "hold_s": 900, "wait_s": 70}'
run shrink-ref1 qwen-image-2/generate prompt="a paper boat on a pond, photo" seed:=$RANDOM megapixels:=1
run shrink-ref2 qwen-image-2/generate prompt="a lantern festival over a river, photo" seed:=$RANDOM megapixels:=1
run shrink-ref-stop ./game.py op=stop cfg='{"name": "ref"}'
verdict=$(python3 -I - "$OUT" <<'PY'
import json, sys
out = sys.argv[1]
rec = lambda n: json.load(open(f"{out}/{n}.json"))
moved = lambda r: sum(v["stats"]["moved"]["store"]["bytes"] + v["stats"]["moved"]["baked"]["bytes"]
                      for v in r["measurements"]["memory"]["stats"]["varena"])
nxt, ref = rec("shrink-next"), rec("shrink-ref2")
pid = nxt["measurements"]["execution"]["gpus"][0]["pid"]
first = nxt["measurements"]["memory"]["stats"]["varena"][0]["stats"].get("room_first_pass") or {}
start_s = nxt["started_at_ms"] / 1000
last = None  # the machine's last target for this executor before the call began, and what was there for it then
for line in open(f"{out}/shrink-memory.log"):
    t, _, body = line.partition(" ")
    if float(t) > start_s or '"event":"room"' not in body:
        continue
    e = json.loads(body)
    target = (e.get("targets") or {}).get(str(pid)) if "targets" in e else (e.get("call") == pid and e.get("target"))
    if not target:
        continue
    # All the card less H, the game's bytes, the driver's own and the other executors': what we hold and what is free,
    # less H and the others.
    others = sum(v for k, v in (e.get("held") or {}).items() if int(k) != pid)
    last = (target, e["own_bytes"] + e["free_bytes"] - e["headroom_bytes"] - others)
slack = 64 << 20
ok = (last is not None and abs(last[0] - last[1]) <= slack and first.get("limit") is not None
      and first.get("target") is not None and abs(first["target"] - last[0]) <= slack)
print(json.dumps({"executor": pid, "first_pass": first, "last_target": last and last[0], "expected": last and last[1],
                  "bytes": moved(nxt), "reference_bytes": moved(ref), "room_ok": ok,
                  "bytes_ok": abs(moved(nxt) - moved(ref)) <= 0.1 * moved(ref)}))
PY
)
say "shrink: $verdict"
{ [ "$(status shrink-next)" = completed ] && jq -e '.room_ok and .bytes_ok' <<<"$verdict" > /dev/null; } ||
  fail "shrink: the next call did not run in the room the game left, or moved more than 10% over its reference"

step restart2 machine update "$NAME" --gpu-memory-limit off -y || fail "restart"
m0=$(memlines)
game start-arm '{"name": "start", "gb": 2, "trigger": "executor", "delay_s": 0, "give_up_s": 60, "hold_s": 40}'
run start-sdxl sdxl/generate prompt="a copper kettle on a stove, photo" seed:=$RANDOM
sleep 45
w=$(cozy machine logs "$NAME" memory -n 0 2>/dev/null | tail -n +$((m0 + 1)))
say "start: $(grep '"event":"driver_pids"' <<<"$w" | cut -d' ' -f2- | head -2 | tr '\n' ' ')"
[ "$(status start-sdxl)" = completed ] || fail "start: the SDXL call is $(status start-sdxl)"
! grep -q '"event":"ended_for_room"' <<<"$w" || fail "start: an executor was ended for a game that started beside its start"
last=$(grep '"event":"gpu_pressure_observation"' <<<"$w" | tail -1)
[ -z "$last" ] || grep -q '"known":true' <<<"$last" || fail "start: the GPU's reading stayed unknown after the game exited"
say "PASS: the game had its memory within 2 s idle and mid-job (also $(param big_gb 5) GB); the room grew back when it shrank; a game beside a start was never taken for ours or theirs for life"
