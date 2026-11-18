# tail (cl-1180): a model download's end against its middle, TensorD builds side by side on one rental (the `cpu` SKU: no GPU
# work). Each trial, in `order`: the model uninstalled, the pod's TensorFS store emptied (empty-store.py), the trial's TensorD put on
# (a restart: consecutive trials must differ), then `cozy model install` on the pod. Each trial keeps TensorD's log and a summary
# from it: the download's seconds (first fetch to last landing), the middle's MB/s (10% to 90% of bytes), the seconds of its
# last 1% of bytes, bytes received beyond the objects' own (`floor moved` bytes less those landed), requests, steals and hedges.
# summary.tsv has a row a trial. params:
#   arms     {"<name>": "artifact:<run id>:<artifact>"}: a dev-build's cozy-<sha>, whose cozy-linux-x86_64 is the trial's TensorD
#   order    ["<name>", …]: each trial's arm, in turn
#   model    default qwen/qwen-image-2.1; lane default int8 (about 19 GB)
#   need_s   seconds the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
MODEL=$(param model qwen/qwen-image-2.1) LANE=$(param lane int8) ARMS=$(param arms '{}') ORDER=$(param order '[]')
mapfile -t trials < <(jq -r '.[]' <<<"$ORDER")
[ "${#trials[@]}" -gt 0 ] || fail "no trials in order"
declare -A TENSORD=() VERSION=()
for a in $(jq -r 'keys[]' <<<"$ARMS"); do
  [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name '$a'"
  IFS=: read -r _ rid artifact <<<"$(jq -r --arg a "$a" '.[$a]' <<<"$ARMS")"
  mkdir -p "$OUT/arms/$a"
  gh run download "$rid" -R cozy-creator/cozy-creator -n "$artifact" -D "$OUT/arms/$a" > /dev/null 2>&1 || fail "arm $a: no artifact $artifact in run $rid"
  TENSORD[$a]=$(find "$OUT/arms/$a" -type f -name 'cozy-linux-x86_64' | head -1)
  [ -n "${TENSORD[$a]}" ] || fail "arm $a: no cozy-linux-x86_64 in $artifact"
  chmod +x "${TENSORD[$a]}"; VERSION[$a]=$("${TENSORD[$a]}" --version | awk '{print $2}')
  say "arm $a: TensorD ${VERSION[$a]}"
done
for i in "${!trials[@]}"; do
  [ -n "${TENSORD[${trials[$i]}]:-}" ] || fail "order names no arm '${trials[$i]}'"
  [ "$i" = 0 ] || [ "${trials[$i]}" != "${trials[$((i - 1))]}" ] || fail "trials $i and $((i + 1)) run one arm: an update to the same TensorD restarts nothing"
done
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 2400)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
printf 'trial\tarm\tversion\tseconds\tgb\tmiddle_mb_s\tlast_1pct_s\textra_mb\trequests\tsteals\thedges\n' > "$OUT/summary.tsv"
for i in "${!trials[@]}"; do
  ARM=${trials[$i]} n=$((i + 1)); D=$OUT/trial-$n-$ARM; mkdir -p "$D"
  step "uninstall-$n" model uninstall "$MODEL" --on "$NAME" -y || say "trial $n: $MODEL wasn't uninstalled: $(tail -c 200 "$OUT/uninstall-$n.txt")"
  # A connect that times out is asked again: RunPod's SSH port dropped one between trials (gekkouga, 44218058209).
  for a in 1 2 3; do
    T=600 pod "cat > /tmp/reset.py && python3 /tmp/reset.py" < "$(dirname "$0")/empty-store.py" > "$D/reset.json" 2>&1 && break
    grep -qE '^ssh: connect to host .*(timed out|refused)' "$D/reset.json" && [ "$a" -lt 3 ] ||
      fail "trial $n: the pod's store wasn't emptied: $(tail -c 300 "$D/reset.json")"
    say "trial $n: $(head -c 200 "$D/reset.json") (attempt $a/3): asking again in 15s"; sleep 15
  done
  step "tensord-$n" machine update "$NAME" --tensord "${TENSORD[$ARM]}" -y || fail "trial $n: the pod refused arm $ARM's TensorD"
  v=$(cozy machine show "$NAME" --json | jq -r .status.version)
  [ "$v" = "${VERSION[$ARM]}" ] || fail "trial $n: the pod runs TensorD $v, not arm $ARM's ${VERSION[$ARM]}"
  BEGAN=$(date +%s) ASK=$(date +%s%3N)
  step "install-$n" model install "$MODEL" --lane "$LANE" --on "$NAME" -y || fail "trial $n: the download failed"
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$D/tensord.log" 2>&1 || fail "trial $n: no TensorD log"
  python3 -I - "$D/tensord.log" "$ASK" > "$D/summary.json" <<'PY' || fail "trial $n: no summary: $(tail -c 300 "$D/summary.json")"
import json, re, sys
from datetime import datetime
log, ask = sys.argv[1], int(sys.argv[2])
at = lambda line: datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp()
fetched, landed, floor, settled, hedged = [], [], 0, {}, 0
for line in open(log, errors="replace"):
    try:
        t = at(line)
    except ValueError:
        continue
    if t * 1000 < ask or "fetch::download" not in line:
        continue
    if m := re.search(r": fetching object=\w+ bytes=(\d+)", line):
        fetched.append(t)
    elif m := re.search(r": landed object=\w+ bytes=(\d+)", line):
        landed.append((t, int(m[1])))
    elif m := re.search(r": floor moved .* to=none .* bytes=(\d+)", line):
        floor += int(m[1])
    elif "downloads settled" in line:
        settled = {k: int(v) for k, v in re.findall(r" (requests|steals|hedges)=(\d+)", line)}
    hedged += "asked again on another connection" in line
landed.sort()
total = sum(b for _, b in landed)
def reached(share):
    acc = 0
    for t, b in landed:
        acc += b
        if acc >= share * total:
            return t
start, end = min(fetched), landed[-1][0]
print(json.dumps({"seconds": round(end - start, 2), "gb": round(total / 1e9, 2),
    "middle_mb_s": round(0.8 * total / max(reached(0.9) - reached(0.1), 1e-3) / 1e6, 1), "last_1pct_s": round(end - reached(0.99), 2),
    "extra_mb": round((floor - total) / 1e6, 2) if floor else None, "objects": len(landed), "hedge_lines": hedged, **settled}))
PY
  say "trial $n ($ARM): $(jq -c . "$D/summary.json")"
  jq -r --arg n "$n" --arg a "$ARM" --arg v "${VERSION[$ARM]}" '[$n, $a, $v, .seconds, .gb, .middle_mb_s, .last_1pct_s, .extra_mb, .requests, .steals, .hedges] | @tsv' \
    "$D/summary.json" >> "$OUT/summary.tsv"
done
column -t "$OUT/summary.tsv" | tee -a "$R"
