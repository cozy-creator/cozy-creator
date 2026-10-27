# One RTX 4090 gate's extra cells (release lane, 2026-10-27), on the session's candidate Runtime (0.34.2 + #1858's
# tile-identity probe): kernel-regret (#2178), the 56 native-memory cells (#2156, run.py through the ordinary CLI),
# and the prebuilt int8 probe's verdict, read from every record (`probe-tiles`). Sourced by gate.sh --cells after the
# product cells; `only` names kernel-regret, native-memory. Proof-only.
. "$HERE/suites/execution/kernel-regret/cells.sh"
if [ -z "$ONLY" ] || [[ $ONLY == *",native-memory,"* ]]; then
  log "native-memory: 56 cells through run.py"
  python3 -I "$HERE/suites/execution/native-memory/run.py" --execute --on "$M" --out "$OUT/native-memory" \
    --seed $((R % 1000000 + 3300000)) > native-memory.txt 2>&1
  c=$? p=$(jq '[.[] | select(.passed)] | length' "$OUT/native-memory/RESULTS.json" 2>/dev/null)
  expect native-memory "$([ "$c" = 0 ] && [ "${p:-0}" = 56 ] && echo PASS || echo FAIL)" \
    "${p:-0} of 56 cells passed (run.py exit $c): $(tail -c 300 native-memory.txt | tr '\n' ' ')"
fi
# The prebuilt int8 probe (cozy-runtime #1858): any record that says a tile's bits differ fails; the int8 GEMM that
# served (analyze.py: cozy-int8 or a FAIL) shows the verdict was ok.
d=$(grep -l 'bits differ' ./*.json ./*.ndjson "$OUT"/native-memory/*.ndjson 2>/dev/null | head -3 | tr '\n' ' ')
v=$(grep -ohE 'int8 (ok|verified)[^"]{0,160}' ./*.json 2>/dev/null | sort -u | head -2 | tr '\n' ' ')
expect probe-tiles "$([ -z "$d" ] && echo PASS || echo FAIL)" "${d:+bits differ in $d}${d:-no record says bits differ}; ${v:-no int8 verdict text in records}"
