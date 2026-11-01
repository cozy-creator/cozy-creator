# dedicated-room: dedicated mode (every pod's) is master's (xs-088 review): a call takes room from idle executors at its
# start (make_room) and while its lane runs (offload to RAM, no attribution); TensorD sends no `room` there. One 16 GB
# card (rtx-a4000), three phases on one pod: the pod's own stack (the last release); the session's cozy as TensorD
# alone (the pod's Runtime and TensorFS: an executor without `room`); then the candidate Runtime and TensorFS too.
# Each phase:
#   kq     Klein bf16 warm and idle, Qwen 1 MP beside it (the 0.36.0 gate case, 41034222864), Klein again: completed
#   twice  a deviceless root (fixtures/pause-twice) calls SDXL twice with a 30 s pause beside the idle models: no memory
#          event in the pause, and the second call replays its loop (reused) when the first phase's did
# and no `room` record in any phase.
# params:
#   runtime, tensorfs  the candidate's, for the last phase (default: that phase is skipped)
#   traffic_gb         GB the provider bills: Klein, Qwen, SDXL and the fixture's environment (default 60)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/pause-twice/package.toml" ] || fail "no fixture checkout"
quote "$(param traffic_gb 60)" 3600
rent

value() { cozy run show "$(runno "$1")" --once --json > "$OUT/$1.json" 2>&1; }
status() { jq -r '.status // empty' "$OUT/$1.json"; }
run() { local n=$1; shift; step "$n" run "$@" --on "$NAME" || fail "$n"; value "$n"; }
memlines() { cozy machine logs "$NAME" memory -n 0 2>/dev/null | wc -l; }
declare -A REUSED
phase() {
  local p=$1 m0 verdict; m0=$(memlines)
  run "$p-klein" flux-3-klein/generate_turbo --lane bf16 prompt="a brass compass on a map, studio photo" seed:=$RANDOM
  run "$p-qwen" qwen-image-2/generate prompt="a lighthouse at dusk, photo" seed:=$RANDOM megapixels:=1
  run "$p-klein2" flux-3-klein/generate_turbo --lane bf16 prompt="a ceramic teapot on linen, studio photo" seed:=$RANDOM
  (cd "$FIX/pause-twice" && run "$p-twice" ./twice prompt="a red bicycle by a canal, photo" seed:=$RANDOM pause_s:=30)
  cozy machine logs "$NAME" memory -n 0 2>/dev/null | tail -n +$((m0 + 1)) > "$OUT/$p-memory.log"
  local s; for s in klein qwen klein2 twice; do [ "$(status "$p-$s")" = completed ] || fail "$p: $s is $(status "$p-$s")"; done
  verdict=$(python3 -I - "$OUT/$p-twice.json" "$OUT/$p-memory.log" <<'PY'
import json, sys
rec = json.load(open(sys.argv[1]))
first, second = rec["calls"][0], rec["calls"][1]
a, b = first["finished_at_ms"] / 1000, second["called_at_ms"] / 1000
pause, rooms = [], 0
for line in open(sys.argv[2]):
    t, _, body = line.partition(" ")
    rooms += '"event":"room"' in body
    if a <= float(t) <= b and any(f'"event":"{e}"' in body for e in ("ended_for_room", "offload", "room", "end_idle")):
        pause.append(json.loads(body)["event"])
loops = [l for l in second["measurements"]["execution"].get("loops", [])]
print(json.dumps({"pause_s": round(b - a, 1), "pause_events": pause, "room_records": rooms,
                  "reused": bool(loops) and all(l.get("reused") for l in loops)}))
PY
  )
  say "$p: $verdict"
  jq -e '.pause_events == [] and .room_records == 0' <<<"$verdict" > /dev/null || fail "$p: dedicated mode moved memory in the root's pause, or sent a room"
  REUSED[$p]=$(jq -r .reused <<<"$verdict")
}

v() { cozy machine show "$NAME" --json | jq -r .status.version; }
say "production: TensorD $(v)"
phase production
step update-tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
[ "$(v)" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $(v), not the session's"
phase tensord
if [ -n "$(param runtime)" ] && [ -n "$(param tensorfs)" ]; then
  step update-full machine update "$NAME" --runtime "$(param runtime)" --tensorfs "$(param tensorfs)" -y || fail "update"
  phase candidate
fi
for p in "${!REUSED[@]}"; do
  [ "${REUSED[production]}" != true ] || [ "${REUSED[$p]}" = true ] || fail "$p: the second call did not replay, production's did"
done
say "PASS: no memory moved in any root's pause and no room sent; Qwen beside idle Klein and both SDXL calls completed in every phase; replays: $(for p in "${!REUSED[@]}"; do printf '%s=%s ' "$p" "${REUSED[$p]}"; done)"
