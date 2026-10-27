# h3-sdxl-qwen: a first call gets back the contexts idle executors keep after offload. The production failure (release
# 40521343073, 22 GiB 4090): H3 Turbo 15 s with a described character reference (BiRefNet and Qwen3.5 executors too),
# then SDXL, then a first Qwen-Image 4MP call ran out of memory while four offloaded idle processes kept 2.4 GB of CUDA
# contexts, because a first call's need is unknown and TensorD ended none of them. One rental (a 4090), driven from a
# runner; the session's cozy carries the candidate TensorD onto the pod. The same sequence must complete, with the
# idle processes ended for the first Qwen call. params:
#   qwen        the Qwen-Image function (default qwen-image-2/generate@0.1.1, the release that ran out)
#   traffic_gb  GB the provider bills (default 120)
. "$(dirname "$0")/lib.sh"
QWEN=$(param qwen qwen-image-2/generate@0.1.1)
status() { cozy run show "$(runno "$1")" --once --json > "$OUT/$1.json" 2>&1; jq -r '.status // empty' "$OUT/$1.json"; }
peak_mib() { jq -r '[.. | objects | select(has("peak_bytes")) | .peak_bytes] | max // 0 | . / 1048576 | floor' "$OUT/$1.json"; }
[ -f "$REF/seed.jpg" ] || fail "no reference image"
quote "$(param traffic_gb 120)" 2700
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

REFS="[{\"name\": \"Mara\", \"kind\": \"character\", \"image\": \"$REF/seed.jpg\"}]"
step h3 run minimax-h3/ref2va_turbo --on "$NAME" references:="$REFS" seed:=$RANDOM duration_s:=15 resolution=768p \
  prompt="<Mara> walks along a rain-wet pier at dusk, gulls overhead, the camera tracking beside her." || fail "h3"
[ "$(status h3)" = completed ] || fail "h3 $(jq -r '.status' "$OUT/h3.json"): $(jq -r '.reason.message // empty' "$OUT/h3.json" | cut -c1-300)"
step sdxl run sdxl/generate --on "$NAME" prompt="a copper kettle on a stove, morning light, photo" seed:=$RANDOM || fail "sdxl"
[ "$(status sdxl)" = completed ] || fail "sdxl did not complete"
base=$(cozy machine logs "$NAME" memory -n 100000 2>/dev/null | wc -l)
step qwen run "$QWEN" --on "$NAME" prompt="This is an RGBA image with transparency. A glass lantern with a candle inside, semi-transparent with soft reflections. The image has alpha channel and the background is transparent." seed:=$RANDOM
qs=$(status qwen)
cozy machine logs "$NAME" memory -n 100000 > "$OUT/memory.log" 2>&1
tail -n +"$((base + 1))" "$OUT/memory.log" > "$OUT/memory-qwen.log"
ended=$(grep -c '"event":"ended_for_room"' "$OUT/memory-qwen.log")
firsts=$(grep '"event":"ended_for_room"' "$OUT/memory-qwen.log" | grep -c '"need":null')
say "qwen ($QWEN) $qs, driver peak $(peak_mib qwen) MiB; idle processes ended for it: $ended ($firsts for a first call)"
[ "$qs" = completed ] || fail "qwen $qs: $(jq -r '.reason.message // empty' "$OUT/qwen.json" | cut -c1-400)"
[ "$ended" -ge 1 ] || fail "qwen completed but no idle process was ended for it (the card had room; the cell did not exercise the fix)"
say "PASS: H3 -> SDXL -> $QWEN completed on $(grep -oE 'GPU 0 .*' "$OUT/show.txt" | head -1); $ended offloaded idle process(es) ended for the first Qwen call"
