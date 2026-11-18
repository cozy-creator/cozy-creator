# video-mentions (se-141, se-164; a proof branch's, never merged): LTX-2.5's and PAN2's boxes take their pictures by mention, the first
# picture mentioned the first frame. Paul, 11-17: "It should have a reference text like <Am> like every other model, and then it shows them in
# the background of the main box. It doesn't need a new box underneath." A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build
# or rc-dev run of rc/<x>), its first-party packages from the packages_refs checkout `tree`, as orbit.sh's (xs-195).
# The page (web/video-mentions.spec.js, gpu.config.js) draws each tool's box mentioning <photo> beside one written with the fields it had
# before (prompt, first_frame, duration_s), at rest and in Focus, paper and dark; without `rent`, that on this runner's machine is all ($0).
# Rented: one $SKU, moved to the RC's builds, then each tool once through `cozy run`, release-gate/ref's picture a run reference its
# instructions mention first; each video's frame 0 held against the picture, scaled and centre-cropped to it (SSIM), and its run record.
# params:
#   tree      the packages_refs checkout the packages run from (required)
#   seconds   what the cap must buy (default 3600)
#   ltx_s     LTX-2.5's clip, seconds (5)        pan2_s   PAN2's (3)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web PICTURE=$REF/seed.jpg
for tool in jq node npm google-chrome ffmpeg ffprobe; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$PICTURE" ] || fail "no $PICTURE"
TREE=$(param tree '') LTX_S=$(param ltx_s 5) PAN2_S=$(param pan2_s 3)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/ltx-2.5/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/pan2/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
for p in ltx-2.5/ltx_2_5 pan2/pan2; do
  grep -q 'instructions: Prompt\[' "$SOURCE/$p/__init__.py" || fail "$p in $TREE takes no instructions: not the change"
done
say "packages from $(git -C "$SOURCE" rev-parse --short HEAD): ltx-2.5 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/ltx-2.5/pyproject.toml"), pan2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/pan2/pyproject.toml")"
for p in ltx-2.5 pan2; do kernels "$SOURCE/$p"; done
# A dev home, its first-party packages from the checkout.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"

# The boxes: each tool's, mentioning the picture first, and one with the fields it had before, never rendered.
LTX_TEXT="The lighthouse's beam sweeps across the dusk sky as waves roll onto the rocks below, and the camera pushes in slowly. Surf and wind."
PAN2_TEXT="Waves roll onto the rocks below the lighthouse while clouds drift across the sky. One continuous wide shot."
BOXES=$(jq -cn --arg l "$LTX_TEXT" --arg p "$PAN2_TEXT" --argjson ls "$LTX_S" --argjson ps "$PAN2_S" '[
  {name: "ltx", tool: "ltx-2.5/generate", fields: {instructions: "<photo> \($l)", duration: $ls}},
  {name: "pan2", tool: "pan2/generate", fields: {instructions: "<photo> \($p)", duration: $ps}},
  {name: "ltx_old", tool: "ltx-2.5/generate", fields: {prompt: $l, first_frame: "<photo>", duration_s: $ls}},
  {name: "pan2_old", tool: "pan2/generate", fields: {prompt: $p, first_frame: "<photo>", duration_s: $ps}}]')
page() {
  local code
  say "page: the boxes"
  env VM_BOXES="$BOXES" VM_PICTURE="$PICTURE" HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY="$(command -v cozy)" \
    CHROME="$(command -v google-chrome)" OUT="$OUT" ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" \
    node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" -g 'video mentions: the boxes to look at' > "$OUT/page.log" 2>&1
  code=$?
  cp "$OUT/web/report.json" "$OUT/web/report-page.json" 2> /dev/null
  node "$WEB/report.js" "$OUT/web/report-page.json" video-mentions > "$OUT/page.report" 2>&1
  sed 's/^/    /' "$OUT/page.report" >> "$R"
  say "page: $(find "$OUT/web/video-mentions" -name '*.png' 2> /dev/null | wc -l) screenshots; fields as Tools lists them: $(jq -c 'map_values([.fields[] | .field])' "$OUT/web/video-mentions/tools.json" 2> /dev/null)"
  [ "$code" = 0 ] || { say "FAIL page: exit $code ($(grep -m1 -E '^FAILED|Error' "$OUT/page.report" | cut -c1-300))"; RC=1; }
}

RENTING=$RENT
RENT=true quote "$(param seconds 3600)"
cozy machine update local -y > "$OUT/local-update.txt" 2>&1 || say "cozy machine update local: $(tail -c 300 "$OUT/local-update.txt")"
for _ in $(seq 120); do [ "$(cozy machine show local --json 2> /dev/null | jq -r '.rc.state // empty')" = current ] && break; sleep 5; done
page
if [ "$RENTING" != true ]; then
  say "pre-flight: nothing rented; the boxes drawn on this runner's machine"
  exit "$RC"
fi
rent
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"

size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$1"; }
# first <label> <video>: frame 0 against the picture, scaled and centre-cropped to the video as the packages take it (SSIM), and
# frame 0 beside the picture.
first() {
  local s=$(size "$2")
  ffmpeg -v error -i "$2" -i "$PICTURE" -lavfi "[1:v]scale=${s%x*}:${s#*x}:force_original_aspect_ratio=increase,crop=${s%x*}:${s#*x},format=yuv420p[p];[0:v]select=eq(n\,0),format=yuv420p[v];[v][p]ssim=stats_file=$OUT/ssim-$1.txt" -frames:v 1 -f null - ||
    { say "FAIL $1: no SSIM"; RC=1; return; }
  ffmpeg -v error -y -i "$2" -i "$PICTURE" -filter_complex "[0:v]select=eq(n\,0),scale=-2:360[a];[1:v]scale=${s%x*}:${s#*x}:force_original_aspect_ratio=increase,crop=${s%x*}:${s#*x},scale=-2:360[b];[b][a]hstack" -frames:v 1 "$OUT/$1-frame0.jpg"
  say "$1: frame 0 vs the picture: SSIM $(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) print substr($i, 5) }' "$OUT/ssim-$1.txt" | head -1)"
}
# render <label> <tool> <text> <seconds> <seed>: one run through `cozy run`, its picture the run's reference <photo>.
render() {
  local made n
  mkdir -p "$OUT/$1"
  step "$1" run "$2" instructions="<photo> $3" duration:="$4" "references:=$(jq -cn --arg f "$PICTURE" '[{name: "photo", image: $f}]')" \
    seed:="$5" "${@:6}" --on "$NAME" -y -o "$OUT/$1" || { say "FAIL $1: $(tail -c 300 "$OUT/$1.txt" | tr '\n' ' ')"; RC=1; }
  n=$(runno "$1")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null
  made=$(find "$OUT/$1" -name '*.mp4' | head -1)
  [ -n "$made" ] || { say "FAIL $1: no video"; RC=1; return; }
  say "$1: run ${n:-?}, $(size "$made"), $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$made") frames, exec $(jq -r '[.. | .execution_ms? // empty][0] // 0 | . / 1000' "$OUT/$1.json" 2> /dev/null) s"
  jq -c '[.. | objects | select(has("prompt") and has("seed") and has("width"))][0] // {} | {width, height, frame_count, seed, prompt}' "$OUT/$1.json" 2> /dev/null | sed "s/^/    $1 result: /" >> "$R"
  jq -r '[.. | .warnings? // empty | .[]?] | unique | .[]' "$OUT/$1.json" 2> /dev/null | sed "s/^/    $1 note: /" >> "$R"
  first "$1" "$made"
}
render ltx ltx-2.5/generate "$LTX_TEXT" "$LTX_S" 20261117 resolution=720p
render pan2 pan2/generate "$PAN2_TEXT" "$PAN2_S" 20261117
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## video-mentions on $NAME ($SKU)"; grep -E '^[0-9:]+ (ltx|pan2|FAIL|page|on the RC)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
