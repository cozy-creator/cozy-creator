# install-share (cl-1007, Paul 11-15: "instead of pausing the download, wouldn't it be better to just leave extra room while another
# install is happening"; cl-1001: hidream's install files sat 10 min behind its own weights): one rental, a release's cold first run,
# its install and its weights together, once per TensorD arm, each from an emptied store with the package uninstalled and none of
# the derived state (stage memos, learned peaks, kernels) an earlier arm left (lib.sh `derived_state`, a census after its TensorD
# starts: tfs-299), on a seed of its own (the same request on the same builds is the call memo's). Each arm keeps
# the run's record, its output stamped line by line, TensorD's log and summary.json: the install's span, the weights' MB/s each 5 s
# across it (what TensorD wrote, read on the pod each second), when they were done, the first output, the link's best 5 s, the
# install's Hub files (software, order 1) and the share's lines (cl-1007). params:
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
# Each second on the pod: what TensorD (`cozy machine serve`) has written, its weights landing, and what the link received.
SAMPLE_PY=$(cat <<'PY'
import os, time
out = open("/tmp/share-samples.txt", "a")
while not os.path.exists("/tmp/share.stop"):
    wrote = 0
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            if b"machine\0serve" in open(f"/proc/{pid}/cmdline", "rb").read():
                wrote += sum(int(line.split()[1]) for line in open(f"/proc/{pid}/io") if line.startswith("wchar:"))
        except OSError:
            pass
    rows = open("/proc/net/dev").read().splitlines()[2:]
    received = sum(int(row.split(":", 1)[1].split()[0]) for row in rows if row.split(":", 1)[0].strip() != "lo")
    out.write(f"{time.time() * 1000:.0f} {wrote} {received}\n"); out.flush()
    time.sleep(1)
PY
)
T=60 pod "rm -f /tmp/share.stop /tmp/share-samples.txt" < /dev/null || fail "the sampler's files"
(printf '%s' "$SAMPLE_PY" | T=9000 pod "cat > /tmp/share.py && python3 /tmp/share.py 2> /tmp/share.err" > /dev/null 2>&1) &
for i in "${!names[@]}"; do
  ARM=${names[$i]}; D=$OUT/arm-$ARM BEGAN=$(date +%s) RUN_PROJECT=$OUT/project-$ARM
  mkdir -p "$D"
  { run_project "$RUN_PROJECT" && (cd "$RUN_PROJECT" && "$RUN_CLI" --json status); } > "$D/project.json" 2>&1 || fail "arm $ARM: its project"
  T=600 pod "cat > /tmp/reset.py && python3 /tmp/reset.py" < "$(dirname "$0")/empty-store.py" > "$D/reset.json" 2>&1 || fail "arm $ARM: the pod's store wasn't emptied: $(tail -c 300 "$D/reset.json")"
  derived_state "$ARM" reset
  step "uninstall-$ARM" package uninstall "${TARGET%%/*}" --on "$NAME" -y || say "arm $ARM: ${TARGET%%/*} wasn't uninstalled"
  step "tensord-$ARM" machine update "$NAME" --tensord "${TENSORD[$ARM]}" -y || fail "the pod refused arm $ARM's TensorD"
  arm_cold "$ARM"
  args=()
  for w in "${INPUTS[@]}"; do [[ ! $w =~ ^seed:=([0-9]+)$ ]] || w="seed:=$((BASH_REMATCH[1] + i))"; args+=("$w"); done
  ASK=$(date +%s%3N)
  say "step run-$ARM: cozy run $TARGET ${args[*]} --on $NAME"
  # Each line its time: the install's steps are its stages, a project's run says nothing in TensorD's log (#2858).
  (cd "$RUN_PROJECT" && timeout 3000 cozy run "$TARGET" "${args[@]}" --on "$NAME" 2>&1) | while IFS= read -r line; do printf '%s %s\n' "$(date +%s%3N)" "$line"; done > "$D/run.txt"
  tail -4 "$D/run.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"
  n=$( (cd "$RUN_PROJECT" && cozy --json run ls --limit 1) | jq -r '.runs[0].number // empty')
  (cd "$RUN_PROJECT" && cozy run watch "$n" --once --json --full) > "$D/run.json" 2>&1
  [ "$(jq -r .status "$D/run.json" 2> /dev/null)" = completed ] || say "FAIL: arm $ARM: the run didn't complete: $(tail -c 300 "$D/run.txt")"
  # Its weights are fetched inside the request: that is what the session measures.
  state_row "$ARM" "$ARM" cold "$D/run.json" "$D/run.txt"
  why=$(unfit "$ARM" fetches); [ -z "$why" ] || { say "FAIL: arm $ARM: $why"; RC=1; }
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$D/tensord.log" 2>&1 || say "arm $ARM: the pod's TensorD log wasn't read"
  T=120 pod "cat /tmp/share-samples.txt" > "$D/samples.txt" 2> /dev/null < /dev/null || say "arm $ARM: the samples weren't read"
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
try:
    run = json.loads((d / "run.json").read_text())
except ValueError:
    run = {}
first = lambda xs, kind: min((s for s, k, _ in xs if k == kind), default=None)
last = lambda xs, kind: max((s for s, k, _ in xs if k == kind), default=None)
# The install's span: its first `installing` line to the first line after its last.
lines = [(int(t) - ask, text) for t, _, text in (line.partition(" ") for line in (d / "run.txt").read_text(errors="replace").splitlines()) if t.isdigit()]
installing = [i for i, (_, text) in enumerate(lines) if "installing " in text and " on this machine" in text]
install = (lines[installing[0]][0] / 1e3, lines[min(installing[-1] + 1, len(lines) - 1)][0] / 1e3) if installing else None
# Each second's samples on the pod: TensorD's writes (its weights landing) and the link's bytes; a restarted TensorD counts from 0.
samples = [tuple(int(float(x)) for x in row.split()) for row in (d / "samples.txt").read_text(errors="replace").splitlines() if len(row.split()) == 3]
rates = [(b[0] - ask, max(0, b[1] - a[1]) / ((b[0] - a[0]) / 1e3), max(0, b[2] - a[2]) / ((b[0] - a[0]) / 1e3))
         for a, b in zip(samples, samples[1:]) if b[0] > a[0] >= ask - 5000]
def buckets(lo, hi, column):
    out = {}
    for t, *values in rates:
        if lo * 1e3 <= t < hi * 1e3:
            out.setdefault(int(t // 5000) * 5, []).append(values[column])
    return {f"{k:+d}s": round(sum(v) / len(v) / 1e6, 1) for k, v in sorted(out.items())}
done = last(weights, "landed")
link = buckets(-5, 1e9, 1)
print(json.dumps({"status": run.get("status"), "install_s": install and [install[0], install[1], round(install[1] - install[0], 1)],
    "first_output_s": ((run.get("settled_at_ms") or ask) - ask) / 1e3, "preparing_s": ((run.get("started_at_ms") or 0) - (run.get("accepted_at_ms") or 0)) / 1e3,
    "execution_s": (run.get("execution_ms") or 0) / 1e3, "weights_done_s": done, "weights_gb": round(sum(b for _, k, b in weights if k == "landed") / 1e9, 2),
    "weights_mb_s_during_install": install and buckets(install[0], install[1] + 5, 0), "link_best_5s_mb_s": max(link.values(), default=0),
    "files_first_fetch_s": first(software, "fetching"), "files_landed_s": last(software, "landed"), "weights_first_fetch_s": first(weights, "fetching"),
    "share_lines": shared, "whole_lines": whole}, indent=1))
PY
  say "arm $ARM: $(jq -c . "$D/summary.json")"
done
T=60 pod "touch /tmp/share.stop" < /dev/null > /dev/null 2>&1
