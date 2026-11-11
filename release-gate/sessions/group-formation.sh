# group-formation (cr-263): a 2-GPU group that cannot form fails within its bound and leaves its GPUs free; a stall that
# clears forms again; an untouched group renders. H3 Turbo 15 s over both GPUs of one rental with SSH. Before each faulted
# run the machine restarts, so the run forms a new group, and fixtures/group-stall.py, in a root shell on the pod, does
# to GPU 1's executor what a fault would:
#   kill   killed before it joins: the run fails at once, naming GPU 1's seam
#   stop   stopped once it joined GPU 0's store: the run fails within one attempt and its unwind (group_unformed), and
#          no executor holds a GPU afterwards
#   retry  stopped once joined, continued after `cont_s`, inside the attempt's unwind: the group forms on attempt 2 and
#          the run completes
# Then H3 untouched completes, 362 frames. params:
#   runtime     a candidate Runtime wheel: https://…#sha256=<hex> or a file (default: the session's RUNTIME_WHEEL)
#   machine     a machine already rented with SSH and 2 GPUs: no rental, and it is left running (a by-hand proof)
#   provider    the rental's provider (runpod, vast; a dev cozy only)
#   cont_s      when the stopped follower continues: past the attempt (60 s), inside its unwind (10 s) (63)
#   stop_s      the latest a stopped run may fail, from its start (150)
#   cases       which to run, in order (kill stop retry untouched)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
URL=$(param runtime "${RUNTIME_WHEEL:-}") CONT=$(param cont_s 63) STOP_S=$(param stop_s 150)
CASES=" $(param cases 'kill stop retry untouched') "
[ -f "$FIX/group-stall.py" ] || fail "no fixtures/group-stall.py"
command -v ffprobe > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
W=$URL
if [[ $URL == https://* ]]; then
  [[ $URL == *'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}"); W=$(printf '%b' "${W//%/\\x}"); mkdir -p "$OUT/wheel"
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
fi
[ -z "$W" ] || [ -f "$W" ] || fail "no Runtime wheel at $W"
NAME=$(param machine)
if [ -z "$NAME" ]; then
  PROVIDER=$(param provider)
  RENT_ARGS="--ssh --gpus 2${PROVIDER:+ --provider $PROVIDER}"
  quote 5400
  rent
fi
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
jq -e '.status.gpus | length == 2' "$OUT/show.json" > /dev/null || fail "$NAME has no 2 GPUs"
[ -z "$W" ] || step update machine update "$NAME" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{rt: .status.runtime, tfs: .status.tensorfs, gpus: [.status.gpus[].name]}')"
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
pod 'cat > /root/group-stall.py' < "$FIX/group-stall.py" || fail "cannot copy the stall fixture"

# h3 <name>: H3 Turbo 15 s with a new seed over both GPUs; its record in <name>.json, its number in $N, its seconds in $S
# (executing; for a run that never executed, accepted to settled).
h3() {
  jq -n --argjson s "$RANDOM$RANDOM" '{summary: {references: [], text: "A steady tracking shot of a courier cycling a cargo bike through a narrow cobbled street after rain, past cafe tables and a flower stall, into a sunlit square."},
    beats: [{duration: 15, description: {references: [], text: "The courier rings the bell at the flower stall and rolls down the slope into the square."}}],
    soundscape: {references: [], text: "Tires hissing on wet cobbles, a bicycle bell, cafe chatter."}, seed: $s}' > "$OUT/$1.req.json"
  step "$1" run minimax-h3/generate --input "$OUT/$1.req.json" --gpus 2 --on "$NAME" --output "$OUT/$1"
  N=$(grep -m1 -oE '^run [0-9]+' "$OUT/$1.txt" | awk '{print $2}')  # a failed run prints no summary for runno
  cozy run watch "$N" --once --json --full > "$OUT/$1.json" 2>&1
  S=$(jq '(.execution_ms // ((.settled_at_ms // 0) - (.accepted_at_ms // 0))) / 1000 | floor' "$OUT/$1.json")
}
# faulted <name> <mode> [cont]: the machine restarted (no executor kept), the fixture armed on the pod, the run, then
# what the fixture did (and it disarmed).
faulted() {
  cozy machine update "$NAME" --gpu-memory-limit off -y > "$OUT/restart-$1.txt" 2>&1 || fail "restart before $1"
  pod "pkill -f '^python3 /root/group-stall'; nohup python3 /root/group-stall.py $2 ${3:-} > /root/group-stall.log 2>&1 < /dev/null &" ||
    fail "cannot arm the stall fixture"
  sleep 3
  h3 "$1"
  pod "pkill -f '^python3 /root/group-stall'; cat /root/group-stall.log" > "$OUT/$1.stall.txt" 2>&1
  grep -v Warning "$OUT/$1.stall.txt" | sed 's/^/    /' >> "$R"
}
why() { jq -r '[.reason.code, .reason.message, .reason.detail] | map(select(. != null)) | join(": ")' "$OUT/$1.json" | tr '\n' ' ' | cut -c1-"${2:-100000}"; }
# free: no process holds either GPU, and the machine runs nothing.
free() {
  local apps
  apps=$(pod 'nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader' 2> /dev/null | grep -c .)
  cozy machine show "$NAME" --json > "$OUT/free.json" 2>&1
  [ "$apps" = 0 ] && [ "$(jq -r '.running // 0' "$OUT/free.json")" = 0 ]
}

if [[ $CASES == *" kill "* ]]; then
faulted kill kill
grep -q SIGKILL "$OUT/kill.stall.txt" || fail "kill: the fixture killed no follower: $(tail -2 "$OUT/kill.stall.txt")"
[ "$(jq -r .status "$OUT/kill.json")" = failed ] && why kill | grep -q 'closed its seam' ||
  fail "kill: run $N is $(jq -r .status "$OUT/kill.json"): $(why kill)"
say "kill: run $N failed in $S s: $(why kill 300)"
fi

if [[ $CASES == *" stop "* ]]; then
faulted stop stop
grep -q SIGSTOP "$OUT/stop.stall.txt" || fail "stop: the fixture stopped no follower: $(tail -2 "$OUT/stop.stall.txt")"
[ "$(jq -r .status "$OUT/stop.json")" = failed ] && [ "$(jq -r .reason.code "$OUT/stop.json")" = group_unformed ] ||
  fail "stop: run $N is $(jq -r .status "$OUT/stop.json"): $(why stop)"
[ "$S" -le "$STOP_S" ] || fail "stop: run $N failed after $S s, past $STOP_S s"
sleep 10; free || fail "stop: after run $N, $(pod 'nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader' 2>&1 | tr '\n' ' ') holds a GPU; machine runs $(jq -r .running "$OUT/free.json")"
say "stop: run $N failed in $S s, no GPU held after: $(why stop 400)"
say "stop: the machine idles: $(cozy machine ls 2>&1 | awk -v n="$NAME" '$1 == n')"
fi

if [[ $CASES == *" retry "* ]]; then
faulted retry stop "$CONT"
grep -q SIGCONT "$OUT/retry.stall.txt" || fail "retry: the fixture did not continue the follower: $(tail -2 "$OUT/retry.stall.txt")"
grep -q 'attempt 2 of 3' "$OUT/retry.txt" || fail "retry: run $N never formed again: $(why retry)"
[ "$(jq -r .status "$OUT/retry.json")" = completed ] || fail "retry: run $N is $(jq -r .status "$OUT/retry.json"): $(why retry)"
F=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$(ls "$OUT"/retry/*.mp4 2> /dev/null | head -1)" 2> /dev/null)
[ "$F" = 362 ] || fail "retry: run $N's video decodes to ${F:-no} frames, not 362"
say "retry: run $N formed on attempt 2 and completed in $S s"
fi

[[ $CASES == *" untouched "* ]] || { say "PASS: $CASES"; exit 0; }
cozy machine update "$NAME" --gpu-memory-limit off -y > "$OUT/restart-untouched.txt" 2>&1 || fail "restart before untouched"
h3 untouched
[ "$(jq -r .status "$OUT/untouched.json")" = completed ] || fail "untouched: run $N is $(jq -r .status "$OUT/untouched.json"): $(why untouched)"
V=$(ls "$OUT"/untouched/*.mp4 2> /dev/null | head -1)
F=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$V" 2> /dev/null)
[ "$F" = 362 ] || fail "untouched: run $N's video decodes to ${F:-no} frames, not 362"
say "PASS: kill, stop and retry as bounded; untouched run $N completed in $S s, 362 frames; $(jq -c '[.. | objects | select(has("route") and has("degree")) | {route, nccl, check}][0]' "$OUT/untouched.json")"
