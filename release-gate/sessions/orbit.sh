# orbit (cl-1131): a real Orbit on one rental of $SKU, and the page turning it. Paul, 11-17: "videos can be marked as 'orbit' videos (a
# property of the video). In that case, they get a special player; you can grab the image and spin it around, 360-style, as if it were a 3d
# model." A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), its first-party packages from the
# packages_refs checkout `tree`, as Paul's laptop runs them (xs-195). The rental moves to the RC's builds before anything runs on it.
# web/orbit.spec.js's GPU branch (`orbit: a real Orbit on a GPU`, gpu.config.js) renders `orbit/orbit` on the rental from release-gate/ref's
# picture: its version says it's an orbit, at rest it carries the mark, and in Focus a drag turns it, there and in full screen. Then its
# file: every frame a keyframe (ffprobe). Without `rent`, $0: the quote, then the same cell on this runner's machine with the local fixture
# (e2e/cli/project/orbit.py), so the hands are proven before anything is rented. params:
#   tree      the packages_refs checkout the packages run from (required)
# Orbit takes its picture by mention in `instructions` (se-152, Paul: "It should have a reference text like <Am> like every other model"), so
# the box mentions <photo>, and an Orbit box written with the fields orbit had before (picture, subject, duration) is drawn beside it, unrendered,
# for what an old box shows. Then one more turn through `cozy run`, its picture a run reference, every frame a keyframe too.
#   seconds   what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web PICTURE=$REF/seed.jpg
for tool in jq node npm google-chrome ffprobe; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$PICTURE" ] || fail "no $PICTURE"
TREE=$(param tree '')
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/orbit/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
say "orbit from $(git -C "$SOURCE" rev-parse --short HEAD): $(grep -c 'with_metadata({"orbit"' "$SOURCE/orbit/orbit/__init__.py") with_metadata, $(grep -c 'keyframe_interval=1' "$SOURCE/orbit/orbit/__init__.py") keyframe_interval=1"
kernels "$SOURCE/minimax-h3"
# A dev home, its first-party packages from the checkout.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"

# pw <label> [VAR=value…]: the cell's GPU branch, its report as web.sh says it.
pw() {
  local label=$1 code; shift
  say "page $label"
  env "$@" HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY="$(command -v cozy)" CHROME="$(command -v google-chrome)" OUT="$OUT" \
    ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" \
    -g 'orbit: a real Orbit on a GPU' > "$OUT/$label.log" 2>&1
  code=$?
  cp "$OUT/web/report.json" "$OUT/web/report-$label.json" 2> /dev/null
  node "$WEB/report.js" "$OUT/web/report-$label.json" orbit > "$OUT/$label.report" 2>&1
  sed 's/^/    /' "$OUT/$label.report" >> "$R"
  [ "$code" = 0 ] || { say "FAIL page $label: exit $code ($(grep -m1 -E '^FAILED' "$OUT/$label.report" | cut -c1-300))"; RC=1; }
  return "$code"
}
# keys <label>: the take's file, every frame a keyframe.
keys() {
  local said=$OUT/web/orbit-gpu/result.json path frames keyed
  path=$OUT/web/orbit-gpu/take.mp4
  [ -f "$path" ] || { say "FAIL $1: no take's file"; RC=1; return; }
  mv "$path" "$OUT/$1.mp4"; path=$OUT/$1.mp4
  frames=$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$path")
  keyed=$(ffprobe -v error -select_streams v:0 -skip_frame nokey -show_entries frame=pts -of csv=p=0 "$path" | wc -l)
  say "$1: run $(jq -r .run "$said"), $(jq -c .file.metadata "$said"), $frames frames, $keyed keyframes, angles $(jq -c .angles "$said")"
  [ -n "$frames" ] && [ "$frames" = "$keyed" ] || { say "FAIL $1: $keyed of $frames frames are keyframes"; RC=1; }
}

RENTING=$RENT
RENT=true quote "$(param seconds 3600)"
if [ "$RENTING" != true ]; then
  say "pre-flight: nothing rented; the page cell on this runner's machine, the Orbit the local fixture"
  cozy machine update local -y > "$OUT/local-update.txt" 2>&1 || say "cozy machine update local: $(tail -c 300 "$OUT/local-update.txt")"
  for _ in $(seq 120); do [ "$(cozy machine show local --json 2> /dev/null | jq -r '.rc.state // empty')" = current ] && break; sleep 5; done
  pw dry ORBIT_MACHINE=local ORBIT_TOOL="$ROOT/e2e/cli/project/orbit.py/turn" ORBIT_FIELDS='{"frames": 48}'
  keys dry
  exit "$RC"
fi
GUARD_ALSO=own guard
order
ready
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
pw orbit ORBIT_MACHINE="$NAME" ORBIT_TOOL=orbit/orbit ORBIT_PICTURE="$PICTURE" \
  ORBIT_FIELDS='{"instructions": "<photo>"}' ORBIT_OLD='{"picture": "<photo>", "subject": "lighthouse", "duration": 15}'
keys orbit
# The same tool through the CLI: `instructions` mentions the run's own reference.
mkdir -p "$OUT/cli"
step cli run orbit/orbit instructions='<photo>' "references:=$(jq -cn --arg f "$PICTURE" '[{name: "photo", image: $f}]')" seed:=20261117 \
  --on "$NAME" -y -o "$OUT/cli" || { say "FAIL cli: $(tail -c 300 "$OUT/cli.txt" | tr '\n' ' ')"; RC=1; }
made=$(find "$OUT/cli" -name '*.mp4' | head -1)
if [ -n "$made" ]; then
  n=$(runno cli); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/cli.json" 2> /dev/null
  frames=$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$made")
  keyed=$(ffprobe -v error -select_streams v:0 -skip_frame nokey -show_entries frame=pts -of csv=p=0 "$made" | wc -l)
  say "cli: run $n, $(jq -c '[.. | .orbit? // empty][0]' "$OUT/cli.json"), $frames frames, $keyed keyframes, exec $(jq -r '(.execution_ms // 0) / 1000' "$OUT/cli.json") s"
  [ "$frames" = "$keyed" ] || { say "FAIL cli: $keyed of $frames frames are keyframes"; RC=1; }
else
  say "FAIL cli: no video"; RC=1
fi
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## orbit on $NAME ($SKU)"; grep -E '^[0-9:]+ (orbit|FAIL|page)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
