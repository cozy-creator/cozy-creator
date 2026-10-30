# hunyuan-48gb: Hunyuan-Image-3 paging through one 48 GB card, a text-only warm-up and the requests after it (cr-225).
# Its int8 lane (82 GB) streams. Production Runtime 0.35.6 failed the warm-up at step 2/8 with `region is already being
# filled` (L40S, pod-session 41017494524): a prefetch's out-of-memory retry ran inside its handler while the traceback
# still held the fill the first attempt had admitted. Every run must complete with a sane image. One rental (a40 or
# l40s), driven from a runner. params:
#   target      the function: "<pkg>/<fn>[@release]" (hunyuan-image-3/generate_turbo), or "tree:<name>" for that
#               packages_refs checkout's hunyuan-image-3/generate_turbo
#   runtime     a candidate Runtime wheel, https://…#sha256=<hex>, installed with `cozy machine update` (optional)
#   runs        requests after the warm-up (3)
#   max_exec_s  each run's execution bar (900; 0.35.2 on an A40: 8 steps of ~28 s)
#   traffic_gb  GB the provider bills (100)
. "$(dirname "$0")/lib.sh"
T=$(param target hunyuan-image-3/generate_turbo) URL=$(param runtime) RUNS=$(param runs 3) MAX=$(param max_exec_s 900)
if [[ $T == tree:* ]]; then
  T=$PACKAGES/${T#tree:}/hunyuan-image-3/generate_turbo; [ -d "${T%/*}/hunyuan_image_3" ] || fail "no packages checkout ${T%/*}"
fi
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}"); mkdir -p "$OUT/wheel"
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
fi
command -v ffprobe >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
PROMPTS=(
  "一只橘猫趴在窗台上晒太阳，水彩画"
  "A lighthouse keeper in a yellow oilskin coat on wet rocks at dusk, photo"
  "一座雪中的木屋，窗户透出温暖的灯光，油画"
  "A red fishing boat tied to a snowy pier at dawn, photo"
  "A glassblower shaping a glowing vase in a dim workshop, film still"
)
[ "$RUNS" -lt "${#PROMPTS[@]}" ] || fail "runs is at most $((${#PROMPTS[@]} - 1))"
quote "$(param traffic_gb 100)" $((1800 + (RUNS + 1) * MAX))
rent

jq -e '.status.gpus | length == 1 and .[0].memory_bytes > 40e9 and .[0].memory_bytes < 52e9' "$OUT/show.json" >/dev/null ||
  fail "not one 48 GB GPU"
[ -z "$URL" ] || step update machine update "$NAME" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name}')"
S=$RANDOM$RANDOM
for i in $(seq 0 "$RUNS"); do
  n=$([ "$i" = 0 ] && echo warm || echo "run$i")
  step "$n" run "$T" prompt="${PROMPTS[$i]}" prompt_upsampling:=false aspect_ratio=3:4 seed:=$((S + i)) --on "$NAME" --out "$OUT/$n"
  N=$(runno "$n"); [ -n "$N" ] || fail "$n named no run"
  cozy run show "$N" --once --json > "$OUT/$n.json" 2>&1
  say "$n $(jq -r '(.measurements.attribution.steps["generating image"].series // [] | map(.[1] / 1000)) as $s |
    ($s[1:] | sort) as $t | "run \(.number) \(.status), exec \((.execution_ms // 0) / 1000 | floor) s;
    step 1 \($s[0] // 0 | floor) s, steady \(if ($t | length) > 0 then $t[($t | length) / 2 | floor] | floor else 0 end) s
    over \($s | length) steps; peak driver \(([.measurements.memory.driver[]?.peak_bytes] | max // 0) / 1e9 * 100 | floor / 100) GB"' \
    "$OUT/$n.json" | tr -s ' \n' ' ')"
  [ "$(jq -r .status "$OUT/$n.json")" = completed ] || fail "$n run $N: $(jq -r '.reason.message // .status' "$OUT/$n.json" | cut -c1-400)"
  P=$(jq -r '[.output[]?.path | select(endswith(".png"))][0] // empty' "$OUT/$n.json")
  IFS=x read -r w h < <(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$P" 2>/dev/null)
  y=$(ffprobe -v error -f lavfi -i "movie=$P,signalstats" -show_entries frame_tags=lavfi.signalstats.YMIN,lavfi.signalstats.YMAX \
    -of csv=p=0 2>/dev/null | head -1)
  [ "${w:-0}" -ge 512 ] && [ "${h:-0}" -gt "${w:-0}" ] || fail "$n run $N: image ${w:-?}x${h:-?}, not 3:4"
  [ $(( ${y#*,} - ${y%,*} )) -ge 64 ] 2>/dev/null || fail "$n run $N: a flat image (luma ${y:-unread})"
done
say "PASS: Hunyuan-Image-3 on a 48 GB card, a warm-up and $RUNS runs after it, each completed with a ${w}x${h} image"
