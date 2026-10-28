# Runtime 0.34.12 + minimax-h3 0.1.5 + qwen-image-2 0.1.5 pre-roll sanity, one GPU (release lane, proof-only): after the
# runtime suite's h3 and qwen (pins), sdxl, restart, sdxl-p2 and conditional-body, SDXL bf16 twice (the second native),
# the qwen cell's exact request on 0.1.4 (the same bytes: 0.1.5 still decodes the whole 4 MP canvas, no tiles), and the
# h3 cell's attention receipt.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/sdxl-native-twice.sh"
cell qwen-014 "qwen-image-2/generate@0.1.4" seed:=$((R + 5)) prompt="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
if [ -f qwen.json ] && [ -f qwen-014.json ]; then
  a=$(jq -r '.output[0].sha256 // empty' qwen.json) b=$(jq -r '.output[0].sha256 // empty' qwen-014.json)
  expect qwen-015-direct "$([ -n "$a" ] && [ "$a" = "$b" ] && echo PASS || echo FAIL)" "0.1.5 $a, 0.1.4 $b"
fi
if [ -f h3.json ]; then
  st=$(run_get "$OUT" h3 .status)
  expect h3-route "$([ "$st" = completed ] && echo PASS || echo FAIL)" "$st; LoRA $(lora h3); route $(route h3); served $(served h3); exact $(exact_sites h3)"
fi
