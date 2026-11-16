# h3-upscale (se-138): minimax-h3's Upscale on one rental as the page renders it (e2e/page.py), from the 15 s reference shot
# (the gate's Skater request, H3_SEED, as a box so its take keeps its latents), every upscale at `to`. First, on the pod's own
# 0.25.0 machine, minimax-h3@0.12.0's take and its upscale in the executor that made it (`old`). Then, on the candidates:
#   was, now       the take with no prompt, from packages_refs checkouts `was` and `now`: the same text and seed, so the same
#                  video (sha256, framemd5 and per-frame PSNR kept)
#   clip_bare      the take's file as a box (no latents: an uploaded video) with no text at 0.7: it runs, warns, and holds the
#                  source (frames.py regen, the gate's bar)
#   clip_prompt    the same file with the shot's words as the prompt
#   prompt, p909   the take with `prompt` replacing its text, at 0.7 and 0.909: they differ from `now`
# After each take: each process on GPU 0 (nvidia-smi over SSH) and varena's split at the take's end. The candidates' upscales
# start in a fresh TensorD (session 43777750768: one in the executor that rendered the take ran out of memory on a 4090).
# Each upscale is held against its shot by frames.py regen; frames 72 and 288, side by side, in $OUT/frames. params:
#   to        1080p (default), 1440p or 2160p
#   prompt    the overriding prompt
#   runtime   the Runtime's manylinux wheel, https://…#sha256=<hex> (a pure wheel has none of its kernels)
#   tensorfs  optional: TensorFS's wheel, the same way
#   old       false: no 0.25.0 take and upscale first
#   need_s    seconds the cap must buy (5400)
. "$(dirname "$0")/lib.sh"
TO=$(param to 1080p)
OVERRIDE=$(param prompt "A grandmother in a bright red tracksuit and round sunglasses rides a skateboard in a concrete skate park at night, in heavy falling snow under orange floodlights.")
WAS=$PACKAGES/was/minimax-h3 NOW=$PACKAGES/now/minimax-h3 GATE=$(cd "$(dirname "$0")/.." && pwd)
[[ $TO =~ ^(1080|1440|2160)p$ ]] || fail "to $TO"
for t in "$WAS" "$NOW"; do [ -f "$t/pyproject.toml" ] || fail "no packages_refs checkout ${t#"$PACKAGES"/}"; done
grep -q 'prompt: Annotated\[Prompt' "$NOW/h3.py" || fail "now's Upscale takes no prompt"
# wheel <url#sha256=hex>: the wheel fetched into $OUT/wheels, its digest checked; nothing printed when it isn't that.
wheel() {
  [[ $1 == https://*'#sha256='* ]] || return 1
  local f; f=$OUT/wheels/$(basename "${1%%#*}" | sed 's/%2B/+/g'); mkdir -p "$OUT/wheels"
  curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" && [ "$(sha256sum "$f" | cut -c1-64)" = "${1##*sha256=}" ] && echo "$f"
}
URL=$(param runtime); RT=$(wheel "$URL") || fail "runtime: no manylinux wheel at ${URL:-(none named)} (https://…#sha256=<hex>)"
UPDATE=(--runtime "$RT"); URL=$(param tensorfs)
if [ -n "$URL" ]; then TF=$(wheel "$URL") || fail "tensorfs: no wheel at $URL"; UPDATE+=(--tensorfs "$TF"); fi
command -v ffprobe > /dev/null || fail "no ffmpeg"
IFS=$'\t' read -r HERO WHO MOTION _ < <(grep -m1 $'^Skater\t' "$GATE/scenarios.tsv")
[ -n "$MOTION" ] || fail "no Skater row in scenarios.tsv"
FR=(uv run -q --no-project --with numpy python -I "$GATE/frames.py")
J() { jq -e "$@" > /dev/null; }
# check <what> <command…>: a bar; a miss is said and fails the session at its end, after every cell has run.
bad=()
check() { local what=$1; shift; if "$@"; then say "PASS $what"; else say "MISS $what"; bad+=("$what"); fi; }
mkdir -p "$OUT/frames"
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
record() { (cd "$RUN_PROJECT" && cozy --json --full run watch "$(newest_run "$1")" --once); }
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r '.versions[-1].files.video.path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 600 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
GIB='/ 1073741824 * 100 | floor / 100'
# memory <record>: each GPU call's peak (allocated, driver) and varena's allocated/reserved at its end, GiB.
memory() { jq -c "[.calls[]? | select(.measurements.memory? != null) | .measurements.memory as \$m | {function, status,
  peak_allocated: ((\$m.peak_allocated_bytes // 0) $GIB), driver_before: ([\$m.driver[]?.before_bytes] | max // 0 $GIB),
  driver_peak: ([\$m.driver[]?.peak_bytes] | max // 0 $GIB), varena_allocated: ((\$m.stats.varena[0].stats.allocated // 0) $GIB),
  varena_reserved: ((\$m.stats.varena[0].stats.reserved // 0) $GIB), error: .error.message}]" "$1" 2>&1; }
# snapshot <label> <take box>: what holds GPU 0 now, each process, and the take's GPU call's memory at its end.
snapshot() {
  say "$1 GPU now: $(timeout 60 cozy machine ssh "$NAME" "nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader; nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader" 2>&1 | grep -v '^Warning: Permanently added' | paste -sd';')"
  cozy machine show "$NAME" --json > "$OUT/$1-show.json" 2>&1
  record "$2" > "$OUT/$2.json"; say "$1 the take's call: $(memory "$OUT/$2.json")"
}
# up <box> <tool> <fields json> <shot box> [soft]: an Upscale box reading what the fields say, rendered; its record in
# $OUT/<box>.json, held against the shot's video. `soft`: a failed render is said and returns 1.
up() {
  run_page add "$1" --tool "$2" --fields "$3" > "$OUT/add-$1.txt" 2>&1 || fail "add $1: $(cat "$OUT/add-$1.txt")"
  say "render $1 on $NAME"
  local ok=0; run_page render --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1 || ok=1
  record "$1" > "$OUT/$1.json" 2>&1
  local n; n=$(newest_run "$1")
  [ -z "$n" ] || [ "$n" = null ] || (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/$1.log" 2>&1
  if [ "$ok" != 0 ]; then
    say "$1 failed: $(grep -o '"failure": {[^}]*}' "$OUT/render-$1.txt" | tail -1 | cut -c1-500); memory $(memory "$OUT/$1.json")"
    [ "${5:-}" = soft ] && return 1
    fail "$1"
  fi
  local v; v=$(file_of "$1") || fail "$1 made no video"; cp "$v" "$OUT/$1.mp4" || fail "$1's video"
  say "$1: run $n $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,nb_frames -of csv=p=0 "$OUT/$1.mp4"), $(grep -o 'text=[a-z]*' "$OUT/$1.log" | head -1), \
regenerate memoized=$(jq -r '[.calls[]? | select((.function // "") | test("regenerate")) | .memoized] | first' "$OUT/$1.json"), warnings $(jq -c '.result.warnings // []' "$OUT/$1.json")"
  say "$1 memory: $(memory "$OUT/$1.json")"
  say "$1 against $4: $("${FR[@]}" regen "$OUT/$4.mp4" "$OUT/$1.mp4" 2>&1 | tail -1 | tee "$OUT/$1.regen.json")"
}
# shot <box> <generate tool> [soft]: the gate's Skater request (15 s, seed 2026102801) as a box of that tool, rendered; its
# video in $OUT/<box>.mp4.
shot() {
  say "the shot $1: the gate's Skater request (15 s, seed 2026102801) from $2"
  run_page add "$1" --tool "$2" --fields "$(jq -cn --arg d "<$HERO> $MOTION" '{seed: 2026102801, beats: [{duration: 15, description: $d}]}')" \
    > "$OUT/add-$1.txt" 2>&1 || fail "add $1: $(cat "$OUT/add-$1.txt")"
  if ! run_page render --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1; then
    say "$1 failed: $(tail -c 500 "$OUT/render-$1.txt")"; [ "${3:-}" = soft ] && return 1; fail "$1"
  fi
  cp "$(file_of "$1")" "$OUT/$1.mp4" || fail "$1 made no video"
  J '.versions[-1].files.latents.sha256' <<< "$(run_page asset "$1")" || fail "$1 kept no latents"
  say "$1: $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,nb_frames -of csv=p=0 "$OUT/$1.mp4")"
}
# same <a> <b>: the two videos' bytes, else every decoded frame: each one's framemd5 (both streams) and the video's mean
# PSNR, all kept in $OUT (<a>-<b>.psnr.log, each frame's).
same() {
  local a b f p
  a=$(sha256sum < "$OUT/$1.mp4" | cut -c1-64) b=$(sha256sum < "$OUT/$2.mp4" | cut -c1-64)
  for v in "$1" "$2"; do ffmpeg -v error -i "$OUT/$v.mp4" -f framemd5 - | grep -v '^#' > "$OUT/$v.framemd5"; done
  ffmpeg -v error -i "$OUT/$1.mp4" -i "$OUT/$2.mp4" -lavfi "[0:v][1:v]psnr=stats_file=$OUT/$1-$2.psnr.log" -f null - 2> /dev/null
  p=$(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^psnr_avg:/) { v = substr($i, 10); v = (v == "inf") ? 100 : v + 0; s += v; n++ } } END { if (n) printf "%.2f dB over %d frames (inf = 100)", s / n, n }' "$OUT/$1-$2.psnr.log" 2> /dev/null)
  say "$1 sha256 $a; $2 sha256 $b; frames $(wc -l < "$OUT/$1.framemd5")/$(wc -l < "$OUT/$2.framemd5"); PSNR ${p:-unread}" >&2
  [ "$a" = "$b" ] && { echo bytes; return 0; }
  f=$(diff "$OUT/$1.framemd5" "$OUT/$2.framemd5" | grep -c '^<')
  [ "$f" = 0 ] && { echo "frames ($p)"; return 0; }
  echo "differ: $f of $(wc -l < "$OUT/$1.framemd5") frame and audio packets, PSNR $p"; return 1
}
# sheet <name> <frame> <video…>: frame N of each, 640 wide, side by side, as $OUT/frames/<name>-<frame>.jpg.
sheet() {
  local n=$1 f=$2 i=0 inputs=() chain='' labels=''; shift 2
  for v in "$@"; do inputs+=(-i "$OUT/$v.mp4"); chain+="[$i:v]select=eq(n\\,$f),scale=640:360,setsar=1[v$i];"; labels+="[v$i]"; i=$((i + 1)); done
  ffmpeg -v error -y "${inputs[@]}" -filter_complex "${chain}${labels}hstack=inputs=$i" -frames:v 1 -q:v 4 "$OUT/frames/$n-$f.jpg" || say "sheet $n $f failed"
}

kernels "$WAS"; kernels "$NOW"
cp "$REF/seed.jpg" "$OUT/skater.jpg"
run_page import "$OUT/skater.jpg" --name "$HERO" > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
RENT_ARGS=--ssh  # nvidia-smi from the pod's root shell
quote "$(param need_s 5400)"
rent
pod() { say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name}')"; }
pod
if [ "$(param old true)" = true ]; then
  say "0.25.0: minimax-h3@0.12.0 on the pod's own machine, the take then its upscale in the executor that made it"
  if shot take25 minimax-h3/generate@0.12.0 soft; then
    snapshot old take25
    up up25 minimax-h3/upscale@0.12.0 "{\"video\": \"<take25>\", \"to\": \"$TO\"}" take25 soft || true
  fi
fi
step update machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y || fail "update"
pod
shot take "$WAS/generate"
snapshot new take
# A settings-only update drains and restarts TensorD (`--gpu-memory-limit off`, the default), so the upscales start in a
# fresh executor.
step fresh machine update "$NAME" --gpu-memory-limit off -y || fail "the restart"

say "no prompt, was then now: the take's own text, the same request"
up was "$WAS/upscale" "{\"video\": \"<take>\", \"to\": \"$TO\"}" take
up now "$NOW/upscale" "{\"video\": \"<take>\", \"to\": \"$TO\"}" take
check "now: the take's latents, no prompt sent: $(jq -c '{latents: .request.input.video.latents, prompt: .request.input.prompt}' "$OUT/now.json")" \
  J '(.request.input.video.latents // "" | startswith("sha256:")) and (.request.input | has("prompt") | not)' "$OUT/now.json"
check "now: no warning of missing text" J '[.result.warnings[]? | select(startswith("no text"))] | length == 0' "$OUT/now.json"
s=$(same was now); check "was = now: $s" [ "${s%%:*}" != differ ]

say "the take's file as a box (an uploaded video: no latents), with no text at 0.7 and with the shot's words"
run_page import "$OUT/take.mp4" --name clip > "$OUT/import-clip.txt" 2>&1 || fail "import clip: $(cat "$OUT/import-clip.txt")"
up clip_bare "$NOW/upscale" "{\"video\": \"<clip>\", \"to\": \"$TO\", \"strength\": 0.7}" take
check "clip_bare: no latents sent, and it warns: $(jq -c '.result.warnings' "$OUT/clip_bare.json")" \
  J '(.request.input.video.latents == null) and ([.result.warnings[]? | select(startswith("no text to guide"))] | length == 1)' "$OUT/clip_bare.json"
check "clip_bare holds the source (the gate's regen bar)" J '.structure_psnr >= 18 and .structure_ssim >= 0.5 and .flicker <= 2 and .corr >= 0.95' "$OUT/clip_bare.regen.json"
up clip_prompt "$NOW/upscale" "$(jq -cn --arg p "$WHO. $MOTION" --arg to "$TO" '{video: "<clip>", to: $to, prompt: $p}')" take
check "clip_prompt: no latents, no warning of missing text" \
  J '(.request.input.video.latents == null) and ([.result.warnings[]? | select(startswith("no text"))] | length == 0)' "$OUT/clip_prompt.json"

say "the take with an overriding prompt, at 0.7 and 0.909"
P=$(jq -cn --arg p "$OVERRIDE" --arg to "$TO" '{video: "<take>", to: $to, prompt: $p}')
up prompt "$NOW/upscale" "$P" take
check "the prompt reached the call: $(jq -c .request.input.prompt "$OUT/prompt.json")" J --arg p "$OVERRIDE" '.request.input.prompt.text == $p' "$OUT/prompt.json"
s=$(same now prompt); check "prompt differs from now: $s" [ "${s%%:*}" = differ ]
up p909 "$NOW/upscale" "$(jq -c '. + {strength: 0.909}' <<< "$P")" take

for f in 72 288; do
  sheet take-was-now-prompt-p909 "$f" take was now prompt p909
  sheet take-clipprompt-clipbare "$f" take clip_prompt clip_bare
done
for v in take now prompt clip_bare; do ffmpeg -v error -y -i "$OUT/$v.mp4" -vf "select=eq(n\\,288)" -frames:v 1 -q:v 3 "$OUT/frames/$v-288.jpg"; done
[ "${#bad[@]}" = 0 ] || fail "missed: ${bad[*]}"
say "PASS: judge $OUT/frames"
