# h3-upscale (se-138): minimax-h3's Upscale on one rental as the page renders it (e2e/page.py), from the 15 s reference shot
# (the gate's Skater request, H3_SEED, as a box so its take keeps its latents), every upscale at `to`:
#   was, now       the take with no prompt, from packages_refs checkouts `was` and `now`: the same text and seed, so the same
#                  video (decoded frames compared)
#   prompt, p909   the take with `prompt` replacing its text, at 0.7 and 0.909: they differ from `now`
#   clip_prompt    the take's file as a box (no latents: an uploaded video) with the shot's words as the prompt
#   clip_bare      the same with no text at 0.7: it runs and warns, and holds the source (frames.py regen, the gate's bar)
# Each upscale is held against the shot by frames.py regen; frames 72 and 288 of each, side by side, in $OUT/frames. params:
#   to       1080p (default), 1440p or 2160p
#   prompt   the overriding prompt
. "$(dirname "$0")/lib.sh"
TO=$(param to 1080p)
OVERRIDE=$(param prompt "A grandmother in a bright red tracksuit and round sunglasses rides a skateboard in a concrete skate park at night, in heavy falling snow under orange floodlights.")
WAS=$PACKAGES/was/minimax-h3 NOW=$PACKAGES/now/minimax-h3 GATE=$(cd "$(dirname "$0")/.." && pwd)
[[ $TO =~ ^(1080|1440|2160)p$ ]] || fail "to $TO"
for t in "$WAS" "$NOW"; do [ -f "$t/pyproject.toml" ] || fail "no packages_refs checkout ${t#"$PACKAGES"/}"; done
grep -q 'prompt: Annotated\[Prompt' "$NOW/h3.py" || fail "now's Upscale takes no prompt"
W=${RUNTIME_WHEEL:-}; [ -n "$W" ] || fail "no Runtime: dispatch with runtime_ref"
command -v ffprobe > /dev/null || fail "no ffmpeg"
IFS=$'\t' read -r HERO WHO MOTION _ < <(grep -m1 $'^Skater\t' "$GATE/scenarios.tsv")
[ -n "$MOTION" ] || fail "no Skater row in scenarios.tsv"
FR=(uv run -q --no-project --with numpy python -I "$GATE/frames.py")
J() { jq -e "$@" > /dev/null; }
# check <what> <command…>: a bar; a miss is said and fails the session at its end, after every cell has run.
bad=()
check() { local what=$1; shift; if "$@"; then say "PASS $what"; else say "MISS $what"; bad+=("$what"); fi; }
mkdir -p "$OUT/frames"
render() { say "render $1 on $NAME"; run_page render --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1 || fail "$1: $(tail -c 500 "$OUT/render-$1.txt")"; }
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
record() { (cd "$RUN_PROJECT" && cozy --json --full run watch "$(newest_run "$1")" --once); }
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r '.versions[-1].files.video.path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 600 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
# up <box> <tree dir> <fields json>: an Upscale box reading what the fields say, rendered; its record in $OUT/<box>.json.
up() {
  run_page add "$1" --tool "$2/upscale" --fields "$3" > "$OUT/add-$1.txt" 2>&1 || fail "add $1: $(cat "$OUT/add-$1.txt")"
  render "$1"
  record "$1" > "$OUT/$1.json" || fail "$1's record"
  local n; n=$(newest_run "$1")
  (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/$1.log" 2>&1
  local v; v=$(file_of "$1") || fail "$1 made no video"; cp "$v" "$OUT/$1.mp4" || fail "$1's video"
  say "$1: run $n $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,nb_frames -of csv=p=0 "$OUT/$1.mp4"), $(grep -o 'text=[a-z]*' "$OUT/$1.log" | head -1), \
regenerate memoized=$(jq -r '[.calls[]? | select((.function // "") | test("regenerate")) | .memoized] | first' "$OUT/$1.json"), warnings $(jq -c '.result.warnings // []' "$OUT/$1.json")"
  say "$1 against the shot: $("${FR[@]}" regen "$OUT/take.mp4" "$OUT/$1.mp4" 2>&1 | tail -1 | tee "$OUT/$1.regen.json")"
}
# same <a> <b>: the two videos' bytes, else every decoded frame (framemd5 of both streams).
same() {
  [ "$(sha256sum < "$OUT/$1.mp4")" = "$(sha256sum < "$OUT/$2.mp4")" ] && { echo bytes; return 0; }
  ffmpeg -v error -i "$OUT/$1.mp4" -f framemd5 - | grep -v '^#' > "$OUT/$1.framemd5"
  ffmpeg -v error -i "$OUT/$2.mp4" -f framemd5 - | grep -v '^#' > "$OUT/$2.framemd5"
  cmp -s "$OUT/$1.framemd5" "$OUT/$2.framemd5" && { echo frames; return 0; }
  echo "differ: $(diff "$OUT/$1.framemd5" "$OUT/$2.framemd5" | grep -c '^<') of $(wc -l < "$OUT/$1.framemd5") frames"; return 1
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
quote 5400
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

say "the shot: the gate's Skater request (15 s, seed 2026102801) from was, as a box"
run_page add take --tool "$WAS/generate" --fields "$(jq -cn --arg d "<$HERO> $MOTION" '{seed: 2026102801, beats: [{duration: 15, description: $d}]}')" \
  > "$OUT/add-take.txt" 2>&1 || fail "add take: $(cat "$OUT/add-take.txt")"
render take
cp "$(file_of take)" "$OUT/take.mp4" || fail "the take made no video"
J '.versions[-1].files.latents.sha256' <<< "$(run_page asset take)" || fail "the take kept no latents"
say "take: $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,nb_frames -of csv=p=0 "$OUT/take.mp4")"

say "no prompt, was then now: the take's own text, the same request"
up was "$WAS" "{\"video\": \"<take>\", \"to\": \"$TO\"}"
up now "$NOW" "{\"video\": \"<take>\", \"to\": \"$TO\"}"
check "now: the take's latents, no prompt sent: $(jq -c '{latents: .request.input.video.latents, prompt: .request.input.prompt}' "$OUT/now.json")" \
  J '(.request.input.video.latents // "" | startswith("sha256:")) and (.request.input | has("prompt") | not)' "$OUT/now.json"
check "now: no warning of missing text" J '[.result.warnings[]? | select(startswith("no text"))] | length == 0' "$OUT/now.json"
s=$(same was now); check "was = now: $s" [ "${s%%:*}" != differ ]

say "the take with an overriding prompt, at 0.7 and 0.909"
P=$(jq -cn --arg p "$OVERRIDE" --arg to "$TO" '{video: "<take>", to: $to, prompt: $p}')
up prompt "$NOW" "$P"
up p909 "$NOW" "$(jq -c '. + {strength: 0.909}' <<< "$P")"
check "the prompt reached the call: $(jq -c .request.input.prompt "$OUT/prompt.json")" J --arg p "$OVERRIDE" '.request.input.prompt.text == $p' "$OUT/prompt.json"
s=$(same now prompt); check "prompt differs from now: $s" [ "${s%%:*}" = differ ]

say "the take's file as a box (an uploaded video: no latents), with the shot's words and with no text at 0.7"
run_page import "$OUT/take.mp4" --name clip > "$OUT/import-clip.txt" 2>&1 || fail "import clip: $(cat "$OUT/import-clip.txt")"
up clip_prompt "$NOW" "$(jq -cn --arg p "$WHO. $MOTION" --arg to "$TO" '{video: "<clip>", to: $to, prompt: $p}')"
check "clip_prompt: no latents, no warning of missing text" \
  J '(.request.input.video.latents == null) and ([.result.warnings[]? | select(startswith("no text"))] | length == 0)' "$OUT/clip_prompt.json"
up clip_bare "$NOW" "{\"video\": \"<clip>\", \"to\": \"$TO\", \"strength\": 0.7}"
check "clip_bare warns: $(jq -c .result.warnings "$OUT/clip_bare.json")" J '[.result.warnings[]? | select(startswith("no text to guide"))] | length == 1' "$OUT/clip_bare.json"
check "clip_bare holds the source (the gate's regen bar)" J '.structure_psnr >= 18 and .structure_ssim >= 0.5 and .flicker <= 2 and .corr >= 0.95' "$OUT/clip_bare.regen.json"

for f in 72 288; do
  sheet take-was-now-prompt-p909 "$f" take was now prompt p909
  sheet take-clipprompt-clipbare "$f" take clip_prompt clip_bare
done
for v in take now prompt clip_bare; do ffmpeg -v error -y -i "$OUT/$v.mp4" -vf "select=eq(n\\,288)" -frames:v 1 -q:v 3 "$OUT/frames/$v-288.jpg"; done
[ "${#bad[@]}" = 0 ] || fail "missed: ${bad[*]}"
say "PASS: judge $OUT/frames"
