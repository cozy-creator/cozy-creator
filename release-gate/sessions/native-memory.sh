# native-memory: creator #2156's 56 native-memory cells (release-gate/suites/execution/native-memory/run.py --execute),
# one after another on one rental through the ordinary CLI, on the machine's own Runtime. Proof-only. params:
#   seed (a new one per session); traffic_gb (20); need_s (3600)
. "$(dirname "$0")/lib.sh"
C=$(cd "$(dirname "$0")/../.." && pwd)
git -C "$C" sparse-checkout add release-gate/suites/execution/native-memory >/dev/null 2>&1 || fail "sparse checkout"
NM=$C/release-gate/suites/execution/native-memory
[ -f "$NM/run.py" ] || fail "no native-memory suite at $NM"
SEED=$(param seed '')
[[ $SEED =~ ^[0-9]+$ ]] || fail "seed"
python3 -I "$NM/run.py" > "$OUT/cells.json" || fail "dry run"
n=$(jq '.cells | length' "$OUT/cells.json")
[ "$n" = 56 ] && [ "$(jq .gpu_action "$OUT/cells.json")" = false ] || fail "dry run lists $n cells"
grep -q 'cozy-runtime>=0.34,<0.35' "$NM/pyproject.toml" || fail "pins"
say "56 cells, seed $SEED, $(git -C "$C" rev-parse --short HEAD)"
quote "$(param traffic_gb 20)" "$(param need_s 3600)"
rent
python3 -I "$NM/run.py" --execute --on "$NAME" --out "$OUT/cells" --seed "$SEED" > "$OUT/run.txt" 2>&1; code=$?
passed=$(jq '[.[] | select(.passed)] | length' "$OUT/cells/RESULTS.json" 2>/dev/null)
tail -5 "$OUT/run.txt" | cut -c1-400 | sed 's/^/    /' >> "$R"
say "run.py exit $code: ${passed:-0} of 56 cells passed"
[ "$code" = 0 ] && [ "${passed:-0}" = 56 ] || fail "native-memory: ${passed:-0} of 56 passed"
