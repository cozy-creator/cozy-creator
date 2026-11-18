# resolve-behind (cl-1185): a cold run's resolve while another run's big download goes, on a rental of $SKU, this runner's
# Calcifer driving it. Paul, 11-17 (PRO6-37): "for the longest time it was 'finding ...' some image model as its status". Per cozy:
# `cozy model install <big>` downloads a big model as a run's need; once its bytes move, one `cozy run qwen3/write` reads a model new
# to the machine (`small`). Its resolve (stage `resolving <small>` to its next stage) takes about a second, and the big download's
# rate while it resolves is its rate before. With `before` (another dev-build run), that cozy runs first, here and on the rental, then
# this session's, each with its own big and small model, so each is cold.
# Without `rent`, $0: the quote, and the checks before it.
# params:
#   runtime       a cozy-runtime dev-build run id the rental runs (default: none, the RC's)
#   before        a cozy-creator dev-build run id measured first (default: none)
#   big_before, big_after      models `cozy model install` downloads (default minimaxai/minimax-h3, qwen/qwen-image-2.1)
#   small_before, small_after  models the cold resolve reads (default qwen/qwen3.5-0.8b@0.1.0, huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0)
#   seconds       what the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
check() { say "FAIL $*"; [ "${LABEL:-}" = before ] || RC=1; }
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
SRC=$(param runtime '') BEFORE=$(param before '')
COZY_BIN=$(command -v cozy)
cp "$COZY_BIN" "$OUT/after-cozy" || fail "no copy of this session's cozy"
if [ -n "$BEFORE" ]; then
  [[ $BEFORE =~ ^[0-9]+$ ]] || fail "before: a cozy-creator dev-build run id"
  gh run download "$BEFORE" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/before-dl" > /dev/null 2>&1 || fail "no cozy in dev-build run $BEFORE"
  install -m 0755 "$(find "$OUT/before-dl" -name cozy-linux-x86_64 | head -1)" "$OUT/before-cozy" || fail "no cozy-linux-x86_64 in run $BEFORE"
  say "before: $("$OUT/before-cozy" --version), after: $(cozy --version)"
fi
W=''
if [ -n "$SRC" ]; then
  [[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
  W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
fi
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -n "$ON" ] || [ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote "$(param seconds 1800)"
GUARD_ALSO=own guard
rent
# A new pod's boot update settled first; a bundle's pod is settled (its idle clock runs meanwhile).
[ -n "${ON:-}" ] || for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done

# moved <ndjson>: the big download's bytes by time, `<at_ms> <bytes_done>`, from its progress.
moved() { grep '^{' "$1" 2> /dev/null | jq -r 'select(.type == "run.progress") | .progress | select(.bytes_done > 0) | "\(.at_ms) \(.bytes_done)"'; }

# measure <label> <cozy> <big> <small>
measure() {
  local label=$1 binary=$2 big=$3 small=$4 bigpid began ended
  LABEL=$label
  if [ "$(cozy --version)" != "$("$binary" --version)" ]; then
    install -m 0755 "$binary" "$COZY_BIN" && cozy restart > "$OUT/restart-$label.txt" 2>&1 || fail "$label: cozy restart: $(tail -c 300 "$OUT/restart-$label.txt")"
  fi
  local update=(--tensord "$binary")
  [ -z "$W" ] || update+=(--runtime "$W")
  step "update-$label" machine update "$NAME" "${update[@]}" -y || fail "$label: the pod's TensorD and Runtime"
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  say "$label on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime)"' "$OUT/show-$label.json")"
  # The big download, a run's need, going.
  (cd "$RUN_PROJECT" && cozy --json model install "$big" --on "$NAME") > "$OUT/$label-big.ndjson" 2> "$OUT/$label-big.err" &
  bigpid=$!
  for _ in $(seq 300); do
    [ "$(moved "$OUT/$label-big.ndjson" | tail -1 | awk '{print $2}')" -gt $((2 << 30)) ] 2> /dev/null && break
    kill -0 "$bigpid" 2> /dev/null || break
    sleep 1
  done
  say "$label: $big downloading: $(moved "$OUT/$label-big.ndjson" | tail -1)"
  sleep 10
  # The cold resolve, timed from its stages.
  began=$(date +%s%3N)
  (cd "$RUN_PROJECT" && cozy --json run qwen3/write "instructions=One word ($label)." --model "model=$small" max_tokens:=1 --on "$NAME") \
    > "$OUT/$label-small.ndjson" 2> "$OUT/$label-small.err" || check "$label: the cold run failed: $(tail -c 300 "$OUT/$label-small.err")"
  ended=$(date +%s%3N)
  wait "$bigpid" || check "$label: $big's install failed: $(tail -c 300 "$OUT/$label-big.err")"
  python3 -I - "$OUT" "$label" "$small" "$began" "$ended" <<'PY' | tee -a "$R" || check "$label: $(tail -n 1 "$OUT/$label-resolve.txt")"
import json, sys
from pathlib import Path
out, label, small, began, ended = Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
events = [json.loads(l) for l in (out / f"{label}-small.ndjson").read_text().splitlines() if l.startswith("{")]
stages = [(e["at_ms"], e["text"]) for e in events if e.get("type") == "run.log" and e.get("level") == "stage"]
name = small.split("@")[0]
start = next((at for at, text in stages if text.startswith("resolving") and name in text), None)
after = next((at for at, text in stages if start is not None and at > start and not text.startswith("resolving")), None)
waited = [text for at, text in stages if text.startswith("waiting for")]
big = []
for line in (out / f"{label}-big.ndjson").read_text().splitlines():
    event = json.loads(line) if line.startswith("{") else {}
    progress = event.get("progress") or {}
    if event.get("type") == "run.progress" and progress.get("bytes_done", 0) > 0:
        big.append((progress["at_ms"], progress["bytes_done"]))
def rate(t0, t1):
    inside = [(at, b) for at, b in big if t0 <= at <= t1]
    return (inside[-1][1] - inside[0][1]) / max(1, inside[-1][0] - inside[0][0]) / 1e3 if len(inside) > 1 else float("nan")
resolve = (after - start) / 1000 if start is not None and after is not None else float("nan")
before_rate = rate(began - 10000, began)
during = rate(start or began, after or ended)
lines = [f"{label}: resolving {small}: {resolve:.2f} s ({len(stages)} stages; waits said: {waited[:2]})",
         f"{label}: big download {before_rate:.0f} MB/s in the 10 s before, {during:.0f} MB/s while the cold run resolved"]
(out / f"{label}-resolve.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
sys.exit(0 if resolve == resolve and resolve < 3 else 1)
PY
}
[ -z "$BEFORE" ] || measure before "$OUT/before-cozy" "$(param big_before minimaxai/minimax-h3)" "$(param small_before qwen/qwen3.5-0.8b@0.1.0)"
measure after "$OUT/after-cozy" "$(param big_after qwen/qwen-image-2.1)" "$(param small_after huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0)"
cozy machine logs "$NAME" tensord --since 2h -n 0 > "$OUT/tensord.log" 2>&1
{ echo "## resolve-behind on $NAME ($SKU)"; cat "$OUT"/*-resolve.txt 2> /dev/null | sed 's/^/- /'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
