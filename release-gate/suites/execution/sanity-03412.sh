# Runtime 0.34.12 + minimax-h3 0.1.5 pre-roll sanity, one GPU (release lane, proof-only): after the runtime suite's h3
# (pins minimax-h3@0.1.5), sdxl, qwen, restart, sdxl-p2 and conditional-body, SDXL bf16 twice (the second native), and
# the h3 cell's attention receipt.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/sdxl-native-twice.sh"
if [ -f h3.json ]; then
  st=$(run_get "$OUT" h3 .status)
  expect h3-route "$([ "$st" = completed ] && echo PASS || echo FAIL)" "$st; route $(route h3); served $(served h3); exact $(exact_sites h3)"
fi
