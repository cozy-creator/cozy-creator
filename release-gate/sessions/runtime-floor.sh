# runtime-floor: a machine updated to a numbered prerelease is no development machine. Escaped on cozy 0.18.5: run 28075
# put corteo on `--runtime 0.39.2 --tensorfs 0.18.1`, both GitHub prereleases, and a package with a floor of 0.39.4 ran
# on 0.39.2 "as its owner chose", failing at import with a traceback. One pod (any GPU), the session's cozy as TensorD and
# that older prerelease pair; a package whose floor its pair misses takes its own Runtime in its own environment, and the
# machine's pair stays. Params:
#   runtime    the machine's older Runtime prerelease (default 0.39.2)
#   tensorfs   the machine's older TensorFS prerelease (default 0.18.1)
#   traffic_gb GB the provider bills (default 8)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
RUNTIME=$(param runtime 0.39.2) TENSORFS=$(param tensorfs 0.18.1)
[ -f "$FIX/runtime-floor.py" ] || fail "no fixture checkout"
quote "$(param traffic_gb 8)" 1200
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RUNTIME" --tensorfs "$TENSORFS" -y || fail "update"
cozy machine show "$NAME" --json > "$OUT/show.json"
[ "$(jq -r .status.runtime "$OUT/show.json")" = "$RUNTIME" ] || fail "the machine runs Runtime $(jq -r .status.runtime "$OUT/show.json"), not $RUNTIME"
step run run "$FIX/runtime-floor.py/versions" --on "$NAME" || fail "the run failed: $(tail -3 "$OUT/run.txt")"
cozy run show "$(runno run)" --once --json > "$OUT/run.json" 2>&1
HELD=$(jq -r .result.runtime "$OUT/run.json")
python3 -I -c 'import sys; v=[int(p) for p in sys.argv[1].split(".")[:3]]; sys.exit(v < [0, 39, 4])' "$HELD" || fail "the environment holds Runtime $HELD"
[ "$(cozy machine show "$NAME" --json | jq -r .status.runtime)" = "$RUNTIME" ] || fail "the machine's pair moved"
say "PASS: the environment took Runtime $HELD; the machine stays on $RUNTIME"
