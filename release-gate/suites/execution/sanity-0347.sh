# Runtime 0.34.7 pre-roll sanity hook (release lane, proof-only), after the runtime suite's h3, sdxl, restart and
# conditional-body: SDXL bf16 twice (the second native), then Qwen-Image-2 4MP after SDXL (the root release users get).
. "$HERE/suites/execution/sdxl-native-twice.sh"
QP="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
cell qwen-after-sdxl "$(pkg qwen-image-2 generate)" prompt="$QP" seed:=$((R + 441))
