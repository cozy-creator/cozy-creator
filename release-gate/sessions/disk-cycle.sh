# disk-cycle (tfs-150): the cr-297 pod (tatsumi, 11-15) failed fibo-2 with `DiskFullError: … the disk is full` while 294 GiB
# of finished models' weights sat on its disk, held by their idle executors. One cheap GPU pod with a small disk, its TensorD
# the session's cozy: five image models run in turn (about 51 GiB of weights), each leaving its executor idle and warm, then
# the first again. Every run completes: the store evicts least recently used, idle executors ended last to free what they
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
cycle() {
  local name=$1 s
  shift
  step "$name" run "$@" --on "$NAME" seed:=$RANDOM || true
  s=$(status "$name")
  say "$name ($1): $s, $(free_gib) GiB free"
  [ "$s" = completed ] || bad+=("$name $s: $(jq -r '.reason.message // empty' "$OUT/$name.json" | cut -c1-300)")
}
cycle sdxl sdxl/generate prompt="a copper kettle on a stove, morning light, photo"
cycle klein flux.3-klein/generate_turbo prompt="A paper lantern festival over a river at night, long exposure photo" prompt_upsampling:=false
cycle krea krea-2/generate_turbo prompt="A ceramic teapot shaped like a sleeping cat on a wooden table, soft window light, photo"
cycle hidream hidream-o1/generate_turbo prompt="A glass terrarium with moss and tiny mushrooms on a desk, macro photo"
cycle fibo fibo-2/generate_turbo prompt="A lighthouse on a rocky headland under a stormy sky" prompt_upsampling:=false
cycle sdxl-again sdxl/generate prompt="an empty concrete skate park at sunset with long shadows, photo"

cozy machine logs "$NAME" tensord -n 0 > "$OUT/tensord.log" 2>&1
grep -E "tensorfs: evicted [^c]|ended the idle executors|the disk was short|doesn't fit|making room in .* waited|fetch refused .*CAPACITY" "$OUT/tensord.log" | cut -c1-300 >> "$R"
evicted=$(grep -c "tensorfs: evicted [^c]" "$OUT/tensord.log")
[ ${#bad[@]} = 0 ] || fail "$(printf '%s; ' "${bad[@]}")"
[ "$evicted" -ge 1 ] || fail "every run completed but no model was evicted: the disk held them all, so this did not exercise tfs-150"
say "PASS: six runs over a $DISK GB disk completed; $evicted models evicted, $(grep -c 'ended the idle executors' "$OUT/tensord.log") times idle executors ended for room"
