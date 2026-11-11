# runtime-floor: a machine updated to a numbered prerelease is no development machine. Escaped on cozy 0.18.5: run 28075
# put corteo on `--runtime 0.39.2 --tensorfs 0.18.1`, both GitHub prereleases, and a package with a floor of 0.39.4 ran
# on 0.39.2 "as its owner chose", failing at import with a traceback. One pod (any GPU), the session's cozy as TensorD and
# that older prerelease pair; a package whose floor its pair misses takes its own Runtime in its own environment, and the
# machine's pair stays. The update moves TensorD too, so the pod's older TensorD prepares it. Params:
#   runtime    the machine's older Runtime prerelease (default 0.39.2)
#   tensorfs   the machine's older TensorFS prerelease (default 0.18.1)
#   sent       1: that wheel sent as a file (`--runtime <path>`), a development Runtime: the package runs on it, warned
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
RUNTIME=$(param runtime 0.39.2) TENSORFS=$(param tensorfs 0.18.1)
[ -f "$FIX/runtime-floor.py" ] || fail "no fixture checkout"
quote 1200
rent
WHEEL=$RUNTIME SENT=$(param sent)
if [ "$SENT" = 1 ]; then
  WHEELS=$(mktemp -d) WHEEL=$WHEELS/cozy_runtime-$RUNTIME-cp312-abi3-manylinux_2_28_x86_64.whl
  gh release download "v$RUNTIME" -R cozy-creator/cozy-runtime -D "$WHEELS" -p "${WHEEL##*/}" || fail "no ${WHEEL##*/}"
fi
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$WHEEL" --tensorfs "$TENSORFS" -y || fail "update"
cozy machine show "$NAME" --json > "$OUT/show.json"
[ "$(jq -r .status.runtime "$OUT/show.json")" = "$RUNTIME" ] || fail "the machine runs Runtime $(jq -r .status.runtime "$OUT/show.json"), not $RUNTIME"
step run run "$FIX/runtime-floor.py/versions" --on "$NAME" || fail "the run failed: $(tail -3 "$OUT/run.txt")"
cozy run watch "$(runno run)" --once --json --full > "$OUT/run.json" 2>&1
HELD=$(jq -r .result.runtime "$OUT/run.json")
if [ "$SENT" = 1 ]; then
  [ "$HELD" = "$RUNTIME" ] || fail "the environment holds Runtime $HELD, not the $RUNTIME sent"
  grep -q "development Runtime $RUNTIME+" "$OUT/run.txt" || fail "no warning names the development Runtime"
  say "PASS: the package runs on the development Runtime $HELD it was sent, warned"
  exit 0
fi
python3 -I -c 'import sys; v=[int(p) for p in sys.argv[1].split(".")[:3]]; sys.exit(v < [0, 39, 4])' "$HELD" || fail "the environment holds Runtime $HELD"
[ "$(cozy machine show "$NAME" --json | jq -r .status.runtime)" = "$RUNTIME" ] || fail "the machine's pair moved"
say "PASS: the environment took Runtime $HELD; the machine stays on $RUNTIME"
