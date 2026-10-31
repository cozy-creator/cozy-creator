# upkeep-wake (cl-697): a GPU request that arrives while TensorD ends an idle executor for host room runs once that
# executor is gone. Escaped on 0.15.0: minimax-h3/upscale 23697 waited 8 min on a free 4090. `host_short` ended the
# shot's idle executor under an upkeep permit while the upscale's SeedVR2 child became ready; the child's dispatch pass
# saw the GPU busy, and the permit's release woke no dispatch. One rental with SSH; real SDXL calls, "gate cl-697" in
# each prompt. Arms: the pod's own TensorD (production), then params.tensord. A try: an SDXL call at a size not run
# before leaves its executor idle; fixtures/squeeze.py, started by a root shell outside cozy's tree, presses host RAM
# until TensorD's host rule ends that executor (the memory log's `end_idle`); the moment that line appears another
# SDXL request is sent, to land in the executor's kill and reap. A request unplaced 60 s after the end is a hang,
# cancelled once captured. Bar: the candidate never hangs. The production arm stops at 2 hangs or `tries`; its hit
# rate is recorded either way.
# params:
#   tensord     the candidate cozy binary (default: the session's cozy)
#   tries       tries per arm (default 10)
#   floor_mb    the room the squeeze leaves (default 256)
#   traffic_gb  GB the provider bills: the image and SDXL (default 25)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/squeeze.py" ] || fail "no fixture checkout"
CAND=$(param tensord "$(command -v cozy)") TRIES=$(param tries 10) FLOOR=$(param floor_mb 256)
[ -x "$CAND" ] || fail "no candidate cozy at $CAND"
quote "$(param traffic_gb 25)" 4800
rent

pod() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
cozy machine ssh "$NAME" -- 'cat > /root/squeeze.py' < "$FIX/squeeze.py" 2>> "$OUT/ssh.err" || fail "copy the squeeze"
pod 'command -v python3' > /dev/null || fail "no python3 on the pod"
pod 'g=/sys/fs/cgroup$(sed -n "s/^0:://p" /proc/self/cgroup); grep -E "MemTotal|MemAvailable" /proc/meminfo;
  while [ "${g#/sys/fs/cgroup}" != "$g" ] || [ "$g" = /sys/fs/cgroup ]; do echo "$g max=$(cat $g/memory.max 2>/dev/null) high=$(cat $g/memory.high 2>/dev/null)"; [ "$g" = /sys/fs/cgroup ] && break; g=${g%/*}; done' > "$OUT/host.txt"
say "pod memory: $(tr '\n' ' ' < "$OUT/host.txt" | cut -c1-400)"
# Without a container limit the squeeze would press the provider's whole host.
grep -qE 'max=[0-9]|high=[0-9]' "$OUT/host.txt" || fail "the pod's cgroups set no memory limit: the squeeze would press the whole host"

SUBJECTS=("a red barn in snow" "a lighthouse at dusk" "a fox in tall grass" "a tram on a rainy street" "a teapot on a sill"
  "a sailboat at dawn" "a stone bridge in fog" "an owl on a fence post" "a market stall of lemons" "a cabin by a lake"
  "a cat on a windowsill" "a desert road at noon" "a koi pond in autumn" "a bicycle by a canal" "a snowy pine forest")
# Each idle call is a size not run before (an aspect ratio, at 1 or 1.25 MP): its executor's growth is unforeseen, so
# TensorD's host rule keeps room for it, and the squeeze can press under that. A size it has measured settles it away.
SHAPES=() SHAPE=0
for mp in 1 1.25; do for a in 1:1 9:7 7:9 3:2 2:3 16:9 9:16 2:1 1:2 21:9 9:21 3:1 1:3; do SHAPES+=("$a $mp"); done; done
sdxl() {  # sdxl <what> [new]: CALL, one real SDXL request's arguments; `new`: the next size not run before
  local a=1:1 mp=1
  [ "${2-}" != new ] || { read -r a mp <<<"${SHAPES[$SHAPE]}"; SHAPE=$((SHAPE + 1)); }
  CALL=(run sdxl/generate "prompt=gate cl-697 $1: ${SUBJECTS[$((RANDOM % ${#SUBJECTS[@]}))]}, photo" "seed:=$RANDOM$RANDOM"
    "aspect_ratio=$a" "megapixels:=$mp" --on "$NAME")
}
show() { cozy run show "$1" --once --json > "$2" 2>&1; }

sdxl "first call, installs SDXL" new; step first "${CALL[@]}" || fail "the first SDXL call"
LOG=$(pod 'find / \( -path /proc -o -path /sys -o -path /dev \) -prune -o -name memory.log -print 2>/dev/null' | grep '/logs/memory.log$' | head -1)
[ -n "$LOG" ] || fail "no memory.log on the pod"
say "memory log $LOG"

# try <arm> <i>: 0 the late request ran, 1 it hung (cancelled), 2 no end_idle came under the squeeze (no verdict).
try() {
  local n=$1-$2 line r t0 s lp placed=''
  sdxl "$1 try $2, the call whose executor goes idle" new; step "$n-idle" "${CALL[@]}" || fail "$n: the idle call failed"
  exec {fd}< <(cozy machine ssh "$NAME" -- "python3 -I /root/squeeze.py $FLOOR 300 $LOG" < /dev/null 2> "$OUT/$n-squeeze.err")
  read -r line <&"$fd"
  case $line in *'"event":"end_idle"'*) ;; *) exec {fd}<&-
    cozy machine logs "$NAME" memory -n 0 > "$OUT/$n-memory.log" 2>&1
    say "$n: no verdict, no end_idle under the squeeze: $line ($(tail -1 "$OUT/$n-squeeze.err"))"; return 2;; esac
  sdxl "$1 try $2, sent as the host rule ended the idle executor"
  cozy "${CALL[@]}" > "$OUT/$n-late.txt" 2>&1 & lp=$!
  t0=$(date +%s); exec {fd}<&-
  echo "$line" > "$OUT/$n-end.txt"
  until r=$(runno "$n-late"); [ -n "$r" ] || [ $(( $(date +%s) - t0 )) -ge 60 ]; do sleep 0.5; done
  [ -n "$r" ] || fail "$n: the late request named no run: $(tail -3 "$OUT/$n-late.txt")"
  while [ $(( $(date +%s) - t0 )) -lt 60 ]; do
    show "$r" "$OUT/$n-late.json"; s=$(jq -r '.started_at_ms // empty' "$OUT/$n-late.json")
    [ -z "$s" ] || { placed=$s; break; }
    sleep 2
  done
  cozy machine logs "$NAME" memory -n 400 > "$OUT/$n-memory.log" 2>&1
  local te ts acc
  te=$(awk '{print $1}' "$OUT/$n-end.txt") acc=$(jq -r '.accepted_at_ms // 0' "$OUT/$n-late.json")
  ts=$(awk -v te="$te" '$1 >= te && /"event":"host_short"/ {print $1; exit}' "$OUT/$n-memory.log")
  local when; when=$(awk -v te="$te" -v ts="${ts:-0}" -v a="$acc" -v p="${placed:-0}" 'BEGIN{
    printf "accepted %+.3f s from end_idle, executor gone %+.3f s, placed %s", a/1000 - te, ts - te, (p > 0 ? sprintf("%+.3f s after it was gone", p/1000 - ts) : "never")}')
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$r" "$when" >> "$OUT/tries.tsv"
  if [ -n "$placed" ]; then
    wait "$lp"; show "$r" "$OUT/$n-late.json"
    [ "$(jq -r .status "$OUT/$n-late.json")" = completed ] || fail "$n: run $r is $(jq -r .status "$OUT/$n-late.json")"
    say "$n: run $r completed: $when"; return 0
  fi
  cozy machine logs "$NAME" tensord -n 200 > "$OUT/$n-tensord.log" 2>&1
  say "$n: HANG run $r: $when; it shows: $(jq -r '.machine_state // .reason // empty | tostring' "$OUT/$n-late.json" | cut -c1-200)"
  cozy run cancel "$r" -y >> "$OUT/$n-cancel.txt" 2>&1 || fail "$n: cancel $r"
  wait "$lp"; return 1
}
arm() {  # arm <name> <stop at hangs>: HANGS, DONE (tries with a verdict) and NONE (without) for it
  local i h=0 c run=0; DONE=0 NONE=0
  for i in $(seq 1 "$TRIES"); do
    try "$1" "$i"; c=$?
    if [ "$c" = 2 ]; then
      NONE=$((NONE + 1)) run=$((run + 1))
      [ "$run" -lt 2 ] || fail "$1: two tries in a row ended no executor under the squeeze (floor $FLOOR MB)"
      continue
    fi
    run=0 DONE=$((DONE + 1)) h=$((h + c))
    [ "$h" -lt "$2" ] || break
  done
  HANGS=$h
}

v0=$(cozy machine show "$NAME" --json | jq -r .status.version)
arm production 2; PH=$HANGS PD=$DONE
say "production TensorD $v0: $PH hangs in $PD tries with a verdict ($NONE without)"
step update machine update "$NAME" --tensord "$CAND" -y || fail "update"
v1=$(cozy machine show "$NAME" --json | jq -r .status.version)
sdxl "first call on the candidate" new; step first-candidate "${CALL[@]}" || fail "the first call on the candidate"
arm candidate 1; CH=$HANGS CD=$DONE
say "candidate TensorD $v1: $CH hangs in $CD tries with a verdict ($NONE without)"
column -t -s $'\t' "$OUT/tries.tsv" | tee -a "$R"
[ "$CH" = 0 ] || fail "the candidate hung: $CH of $CD"
say "PASS: candidate $v1 placed every request that arrived as an idle executor ended ($CD tries); production $v0 hung $PH of $PD"
