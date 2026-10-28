# Ideogram 4 on an Ampere card with no lane named (proof-only): the Hub serves its fp8 lane weight-only at 16-bit compute
# (tensorhub #1367, cozy-creator #2210), and the request completes. Sourced by gate.sh --cells.
cell ideogram "$(pkg ideogram-4 generate)" prompt="$OBJECT on a wooden table, a small paper label reading \"HONEY\", product photo" seed:=$((R + 391))
if [ -f ideogram.json ]; then
  st=$(run_get "$OUT" ideogram .status)
  lanes=$(jq -r '[.. | objects | select(has("lane")) | .lane | strings] | unique | join(",")' ideogram.json 2>/dev/null)
  expect ideogram-fp8 "$([ "$st" = completed ] && [[ ",$lanes," == *",fp8,"* ]] && echo PASS || echo FAIL)" "$st; lanes $lanes"
fi
