# Runtime 0.34.9 + minimax-h3 0.1.4 pre-roll sanity hook (release lane, proof-only), after the runtime suite's h3
# (0.1.2, the root release), sdxl, qwen, restart and conditional-body: SDXL bf16 twice (the second native), then H3 Turbo
# 15 s on the yanked candidate minimax-h3 0.1.4 (exact pin), same request shape as the h3 product cell.
. "$HERE/suites/execution/sdxl-native-twice.sh"
cell h3-014 "minimax-h3/ref2va_turbo@0.1.4" references:="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  seed:=$((R + 451)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION The light shifts to late afternoon."
