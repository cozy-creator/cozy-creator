# install-share (cl-1007, Paul 11-15: "instead of pausing the download, wouldn't it be better to just leave extra room while another
# install is happening"; cl-1001: hidream's install files sat 10 min behind its own weights): one rental, a release's cold first run,
# its install and its weights together, once per TensorD arm, each from an emptied store with the package uninstalled. Each arm keeps
# the run's record, TensorD's log and summary.json: preparing, the install's Hub files (software, order 1), the weights' first and last
# bytes, and the share's lines (cl-1007). params:
#   target     the function (default hidream-o1/generate_turbo), with `inputs`, `field=value` words (default a prompt)
#   arms       [{"name": "hold", "tensord": "artifact:<run id>:<artifact>"}, {"name": "share", "tensord": "…"}], in order
#   link_mbit  the pod's ingress policed to this many Mbit/s (tc over SSH); a pod that refuses it runs as rented
. "$(dirname "$0")/lib.sh"
TARGET=$(param target hidream-o1/generate_turbo) ARMS=$(param arms '[]') LINK=$(param link_mbit '')
read -ra INPUTS <<<"$(param inputs 'prompt=A glass terrarium with moss and tiny mushrooms on a desk, macro photo seed:=20953')"
[[ -z $LINK || $LINK =~ ^[0-9]+$ ]] || fail "link_mbit $LINK"
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] || fail "no arms"
declare -A TENSORD=()
for i in "${!names[@]}"; do
  a=${names[$i]}; [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name '$a'"
  IFS=: read -r _ rid artifact <<<"$(jq -r --argjson i "$i" '.[$i].tensord' <<<"$ARMS")"
  mkdir -p "$OUT/arms/$a"
  gh run download "$rid" -R cozy-creator/cozy-creator -n "$artifact" -D "$OUT/arms/$a" > /dev/null 2>&1 || fail "arm $a: no artifact $artifact in run $rid"
  TENSORD[$a]=$(find "$OUT/arms/$a" -type f -name 'cozy-linux-x86_64' | head -1)
  [ -n "${TENSORD[$a]}" ] || fail "arm $a: no cozy-linux-x86_64 in $artifact"
  chmod +x "${TENSORD[$a]}"
  say "arm $a: TensorD $("${TENSORD[$a]}" --version | awk '{print $2}')"
done
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s $((1800 * ${#names[@]})))"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
if [ -n "$LINK" ]; then
  dev=$(pod "ip route show default | awk '{print \$5; exit}'" < /dev/null 2> /dev/null | tr -d '\r')
  if pod "tc qdisc add dev $dev handle ffff: ingress && tc filter add dev $dev parent ffff: protocol ip u32 match u32 0 0 police rate ${LINK}mbit burst 4m drop flowid :1" \
      < /dev/null > "$OUT/link.txt" 2>&1; then
    say "the pod's link ($dev) is shaped to $LINK Mbit/s"
  else
    say "the pod refused the shaped link: $(tail -c 200 "$OUT/link.txt"); it runs as rented"
  fi
fi
for ARM in "${names[@]}"; do
  D=$OUT/arm-$ARM BEGAN=$(date +%s) RUN_PROJECT=$OUT/project-$ARM
  mkdir -p "$D"
  { run_project "$RUN_PROJECT" && (cd "$RUN_PROJECT" && "$RUN_CLI" --json status); } > "$D/project.json" 2>&1 || fail "arm $ARM: its project"
  T=600 pod "cat > /tmp/reset.py && python3 /tmp/reset.py" < "$(dirname "$0")/empty-store.py" > "$D/reset.json" 2>&1 || fail "arm $ARM: the pod's store wasn't emptied: $(tail -c 300 "$D/reset.json")"
  step "uninstall-$ARM" package uninstall "${TARGET%%/*}" --on "$NAME" -y || say "arm $ARM: ${TARGET%%/*} wasn't uninstalled"
  step "tensord-$ARM" machine update "$NAME" --tensord "${TENSORD[$ARM]}" -y || fail "the pod refused arm $ARM's TensorD"
  ASK=$(date +%s%3N)
  step "run-$ARM" run "$TARGET" "${INPUTS[@]}" --on "$NAME" || say "FAIL: arm $ARM: the run didn't complete: $(tail -c 300 "$OUT/run-$ARM.txt")"
  (cd "$RUN_PROJECT" && cozy run watch "$(runno "run-$ARM")" --once --json --full) > "$D/run.json" 2>&1
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$D/tensord.log" 2>&1 || say "arm $ARM: the pod's TensorD log wasn't read"
  python3 - "$D" "$ASK" > "$D/summary.json" <<'PY'
import json, re, sys
from datetime import datetime
from pathlib import Path
d, ask = Path(sys.argv[1]), int(sys.argv[2])
at = lambda line: int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
software, weights, shared, whole = [], [], 0, 0
for line in (d / "tensord.log").read_text(errors="replace").splitlines():
    try:
        t = at(line)
    except ValueError:
        continue
    if t < ask:
        continue
    found = re.search(r": (fetching|landed) .*bytes=(\d+).*priority=(\d+)", line)
    if found:
        (software if int(found[3]) == 1 else weights if int(found[3]) >= 2 else []).append(((t - ask) / 1e3, found[1], int(found[2])))
    shared += "link shared with an install" in line
    whole += "weights take the whole link" in line
run = json.loads((d / "run.json").read_text() or "{}")
first = lambda xs, kind: min((s for s, k, _ in xs if k == kind), default=None)
last = lambda xs, kind: max((s for s, k, _ in xs if k == kind), default=None)
print(json.dumps({"status": run.get("status"), "preparing_s": ((run.get("started_at_ms") or 0) - (run.get("accepted_at_ms") or 0)) / 1e3,
    "execution_s": (run.get("execution_ms") or 0) / 1e3, "files_first_fetch_s": first(software, "fetching"), "files_landed_s": last(software, "landed"),
    "weights_first_fetch_s": first(weights, "fetching"), "weights_last_landed_s": last(weights, "landed"),
    "weights_gb": round(sum(b for _, k, b in weights if k == "landed") / 1e9, 2), "share_lines": shared, "whole_lines": whole}, indent=1))
PY
  say "arm $ARM: $(jq -c . "$D/summary.json")"
done
