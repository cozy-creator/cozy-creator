# disk-cycle (tfs-150): the cr-297 pod (tatsumi, 11-15) failed fibo-2 with `DiskFullError: … the disk is full` while 294 GiB
# of finished models' weights sat on its disk, held by their idle executors. One cheap GPU pod with a small disk, its TensorD
# the session's cozy: five image models (about 51 GiB of weights), the last two at once, each leaving its executor idle and
# warm, then the first again. Every run completes: the store evicts least recently used, idle executors ended last to free what they
# hold, and the log names what went. params:
#   need_s   seconds the guard must buy (default 2400)
# Dispatch: sku=rtx-4090 disk=40 cap=1 (a disk smaller than the cycle's weights).
. "$(dirname "$0")/lib.sh"
status() { cozy run watch "$(runno "$1")" --once --json --full > "$OUT/$1.json" 2>&1; jq -r '.status // empty' "$OUT/$1.json"; }
free_gib() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '.status.disk_free_bytes // 0 | . / 1073741824 | floor'; }
quote "$(param need_s 2400)"
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v; $(free_gib) GiB free"

bad=()
judge() {
  local name=$1 s
  s=$(status "$name")
  say "$name: $s, $(free_gib) GiB free"
  [ "$s" = completed ] || bad+=("$name $s: $(jq -r '.reason.message // empty' "$OUT/$name.json" | cut -c1-300)")
}
cycle() { step "$1" run "${@:2}" --on "$NAME" seed:=$RANDOM || true; judge "$1"; }
cycle sdxl sdxl/generate prompt="a copper kettle on a stove, morning light, photo"
cycle klein flux.3-klein/generate_turbo prompt="A paper lantern festival over a river at night, long exposure photo" prompt_upsampling:=false
cycle krea krea-2/generate_turbo prompt="A ceramic teapot shaped like a sleeping cat on a wooden table, soft window light, photo"
# Two at once, each needing room: only the idle executors may end for it, never the other's while it runs.
step hidream run hidream-o1/generate_turbo prompt="A glass terrarium with moss and tiny mushrooms on a desk, macro photo" --on "$NAME" seed:=$RANDOM &
sleep 20
step fibo run fibo-2/generate_turbo prompt="A lighthouse on a rocky headland under a stormy sky" prompt_upsampling:=false --on "$NAME" seed:=$RANDOM || true
wait
judge hidream; judge fibo
both=$(jq -s '[.[].settled_at_ms // 0] | max' "$OUT/hidream.json" "$OUT/fibo.json")
cycle sdxl-again sdxl/generate prompt="an empty concrete skate park at sunset with long shadows, photo"

cozy machine logs "$NAME" tensord -n 0 > "$OUT/tensord.log" 2>&1
grep -E "tensorfs: evicted [^c]|ended the idle executors|short for this render|doesn't fit|making room in .* waited|fetch refused .*CAPACITY" "$OUT/tensord.log" | cut -c1-300 >> "$R"
evicted=$(grep -c "tensorfs: evicted [^c]" "$OUT/tensord.log")
# Until both settled, no ended executor held hidream-o1 or fibo-2: theirs served a run.
disturbed=$(python3 - "$OUT/tensord.log" "$both" <<'PY'
import sys
from datetime import datetime
log, until = sys.argv[1], int(sys.argv[2])
for line in open(log, errors="replace"):
    if "ended the idle executors" not in line:
        continue
    try:
        at = datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000
    except ValueError:
        continue
    if at < until and ("hidream" in line or "fibo" in line):
        print(line.strip()[:200])
PY
)
[ ${#bad[@]} = 0 ] || fail "$(printf '%s; ' "${bad[@]}")"
[ -z "$disturbed" ] || fail "an executor serving a run was ended for room: $disturbed"
[ "$evicted" -ge 1 ] || fail "every run completed but no model was evicted: the disk held them all, so this did not exercise tfs-150"
say "PASS: six runs over a $DISK GB disk completed, two of them at once; $evicted models evicted, idle executors ended for room $(grep -c 'ended the idle executors' "$OUT/tensord.log") times, never one serving a run"
