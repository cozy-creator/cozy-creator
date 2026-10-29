# ab-tensord: one rental, the same calls in order under each TensorD arm (proof-only). Each arm after the first puts its
# TensorD on the pod (`cozy machine update --tensord <version>`: the machine restarts, so every arm starts with fresh
# executors), then runs the calls; after each call the pod's GPU memory per process is read (nvidia-smi). params:
#   arms        [{"name": "old", "tensord": "image"}, {"name": "new", "tensord": "0.13.12"}, {"name": "old2", "tensord": "0.13.11"}]
#   calls       [{"label": "sdxl", "fn": "sdxl/generate", "args": ["prompt=…", "seed:=1"]}, …], run in order per arm
#   traffic_gb  GB the provider bills (100); need_s seconds the guard must buy (3600)
. "$(dirname "$0")/lib.sh"
ARMS=$(param arms '[{"name": "image", "tensord": "image"}]') CALLS=$(param calls '[]')
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS"); mapfile -t builds < <(jq -r '.[].tensord' <<<"$ARMS")
mapfile -t labels < <(jq -r '.[].label' <<<"$CALLS")
[ "${#names[@]}" -gt 0 ] && [ "${#labels[@]}" -gt 0 ] || fail "no arms or calls"
for b in "${builds[@]}"; do [[ $b == image || $b =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "tensord $b: image or a release"; done
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param traffic_gb 100)" "$(param need_s 3600)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
call() {
  local label=$1; shift; local fn=$1; shift
  cozy run "$fn" "$@" --on "$NAME" --json --out "$OUT/$label" > "$OUT/$label.ndjson" 2> "$OUT/$label.err"
  local n; n=$(grep -o '"number":[0-9]*' "$OUT/$label.ndjson" | head -1 | cut -d: -f2)
  [ -n "$n" ] || { say "$label: no run ($(tail -c 300 "$OUT/$label.err"))"; return 0; }
  cozy run show "$n" --once --json > "$OUT/$label.json" 2>&1
  T=60 pod "nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader; nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader" \
    > "$OUT/$label.smi" 2>&1 < /dev/null
  say "$label: run $n $(jq -r .status "$OUT/$label.json") $(jq -r '(.execution_ms // 0) / 1000' "$OUT/$label.json") s $(jq -r '.reason.code // ""' "$OUT/$label.json"); GPU $(tr '\n' ';' < "$OUT/$label.smi")"
}
for i in "${!names[@]}"; do
  a=${names[$i]}
  if [ "${builds[$i]}" != image ]; then
    step "update-$a" machine update "$NAME" --tensord "${builds[$i]}" -y || fail "$a: update"
  fi
  v=$(cozy machine show "$NAME" --json | jq -r .status.version); say "$a: pod TensorD $v"
  [ "${builds[$i]}" = image ] || [ "$v" = "${builds[$i]}" ] || fail "$a: the pod runs TensorD $v, not ${builds[$i]}"
  for j in "${!labels[@]}"; do
    mapfile -t args < <(jq -r --argjson j "$j" --arg a "$a" '.[$j].args[] | gsub("@ARM@"; $a)' <<<"$CALLS")
    call "$a-${labels[$j]}" "$(jq -r --argjson j "$j" '.[$j].fn' <<<"$CALLS")" "${args[@]}"
  done
done
say "PASS: ${#names[@]} arms"
