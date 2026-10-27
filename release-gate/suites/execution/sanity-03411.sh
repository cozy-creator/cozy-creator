# Runtime 0.34.11 + qwen-image-2 0.1.4 pre-roll sanity hook (release lane, proof-only), after the runtime suite's h3,
# sdxl, qwen (pins qwen-image-2@0.1.4, 4 MP after H3 and SDXL), restart, sdxl-p2 and conditional-body: SDXL bf16 twice
# (the second native), then the qwen cell's exact request on 0.1.3. The same bytes mean 0.1.4 decoded the whole canvas.
. "$HERE/suites/execution/sdxl-native-twice.sh"
cell qwen-013 "qwen-image-2/generate@0.1.3" seed:=$((R + 5)) prompt="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
if [ -f qwen.json ] && [ -f qwen-013.json ]; then
  a=$(jq -r '.output[0].sha256 // empty' qwen.json) b=$(jq -r '.output[0].sha256 // empty' qwen-013.json)
  expect qwen-014-direct "$([ -n "$a" ] && [ "$a" = "$b" ] && echo PASS || echo FAIL)" "0.1.4 $a, 0.1.3 $b"
fi
