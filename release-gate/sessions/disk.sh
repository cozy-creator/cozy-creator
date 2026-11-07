# disk: the release gate's `disk` suite (suites/disk/cells.sh) on one pod, its TensorD the session's cozy: a run held
# through a full disk never loses its model, the store evicts least recently used models whole until every byte it
# holds is a model `model ls` names, and a render that fits only by evicting runs and says what went (cl-774). The
# suite's install cells (disk-envs, torch-layer) are left out unless `installs` is yes. params:
#   installs    yes runs every cell (default no)
#   need_s      seconds the guard must buy (default 1800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")/.." && pwd)
[ -f "$HERE/suites/disk/cells.sh" ] && [ -f "$HERE/../e2e/run.sh" ] || fail "no disk suite or e2e/run.sh"
quote "$(param need_s 1800)"
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

# The suite in a subshell under the gate's names (gate.sh): its cozy, machine, an object to draw, how it records a
# verdict, and R, which is the gate's seed and this session's results file.
RESULTS=$R
(
  CLI=cozy M=$NAME R=$(( $(date +%s%N) / 1000 % 4000000000 )) OBJECT="a brass compass on a walnut desk"
  DISK_INSTALLS=$([ "$(param installs no)" = yes ] && echo yes || echo no)
  say() { echo "$(date -u +%T) $*" | tee -a "$RESULTS"; }
  log() { say "$*"; }
  expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> "$OUT/expect.tsv"; say "$1: $2 $3"; }
  pkg() { echo "$1/$2"; }
  RUN_CLI=cozy; . "$HERE/../e2e/run.sh"
  . "$HERE/suites/disk/cells.sh"
)

cozy machine logs "$NAME" tensord -n 0 > "$OUT/tensord.log" 2>&1
grep -E 'tensorfs: evicted|ended the idle executors|the disk was short' "$OUT/tensord.log" | cut -c1-300 >> "$R"
failed=$(awk -F'\t' '$2 == "FAIL" {print $1}' "$OUT/expect.tsv" | paste -sd' ')
[ -s "$OUT/expect.tsv" ] || fail "no cell recorded a verdict"
[ -z "$failed" ] || fail "cells failed: $failed"
say "PASS: $(cut -f1 "$OUT/expect.tsv" | paste -sd' ')"
