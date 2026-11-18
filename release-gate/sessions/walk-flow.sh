# walk-flow (0.27.0 launch walkthrough, the coordinator's brief 11-18; Paul 11-18: "the agent uses the actual app, and tries running the
# actual code, as it is intended to be used by a user"): a new user's path end to end on ONE rental, through Calcifer's own page in Chrome
# at a laptop's size (e2e/cli/web/walkflow.spec.js): a project made from the page, an image box and a short H3 Shot from the Tools palette,
# their words typed, $SKU with `gpus` GPUs rented from the Machines panel and preferred, the Shot rendered, the queue and the machine's
# drawer looked at while it runs, the picture rendered while the Shot goes, both again warm, each result seen in Focus, played, downloaded
# by a right click, the rental ended from the page. user-flow.sh's setup (a dev home on the packages checkout), one machine instead of
# its two (the coordinator: one rental of this session at a time). Without `rent`, $0: the page as far as its quote, nothing ordered.
# Proof-branch harness (release/walkthrough-027), never merged. params:
#   tree       the packages_refs checkout the first-party packages run from (required)
#   gpus       the rental's GPUs (default 2)
#   image      the image tool (default flux.3-klein/image)
#   shot_s     the Shot's seconds (default 5)
#   seconds    what the cap must buy (default 3600)
#   quiet_min  a wait fails once nothing it watches has moved for this long (default 20)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
for tool in jq node npm google-chrome ffmpeg ffprobe python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
GPUS=$(param gpus 2) TREE=$(param tree '') IMAGE=$(param image flux.3-klein/image) SHOT_S=$(param shot_s 5)
[[ $GPUS =~ ^[1-8]$ && $SHOT_S =~ ^[0-9]+$ && $SHOT_S -ge 1 && $SHOT_S -le 15 ]] || fail "gpus $GPUS, shot_s $SHOT_S"
FLOW=$OUT/flow
mkdir -p "$FLOW"
bash "$ROOT/e2e/cli/install-vault-driver.sh" fuse3 > "$OUT/fuse.log" 2>&1 || fail "no FUSE: $(tail -c 300 "$OUT/fuse.log")"
SECUREFS=$HOME/.cozy/tools/securefs/$(jq -r .version "$ROOT/crates/project-mount/helper/manifest.json")
if [ ! -x "$SECUREFS/securefs" ]; then
  archive=$(python3 -I "$ROOT/scripts/prepare-securefs.py" --target x86_64-unknown-linux-gnu --out-dir "$RUNNER_TEMP/securefs") || fail "no securefs"
  mkdir -p "$SECUREFS" && chmod 700 "$SECUREFS" &&
    python3 -I -c 'import sys, zipfile; open(sys.argv[2], "wb").write(zipfile.ZipFile(sys.argv[1]).read("securefs"))' "$archive" "$SECUREFS/securefs" &&
    chmod 755 "$SECUREFS/securefs" || fail "securefs not put in $SECUREFS"
fi
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/minimax-h3"; kernels "$SOURCE/${IMAGE%%/*}"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer config not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
# What this runner's Calcifer ordered is this session's: the page orders it.
trap 'code=$?; claim; (exit "$code"); finish' EXIT
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
cozy machine ls > "$FLOW/machine-ls-before.txt" 2>&1 < /dev/null
RENTING=$RENT
RENT=true quote "$(param seconds 3600)" "$SKU:$GPUS"
GUARD_ALSO=own
[ "$RENTING" != true ] || guard
say "walk-flow on cozy $V, a dev home, packages $TREE $(git -C "$SOURCE" rev-parse --short=10 HEAD): $SKU x$GPUS from the page, $IMAGE and a ${SHOT_S} s Shot$([ "$RENTING" = true ] || echo '; pre-flight: the page up to its quote, nothing ordered')"

COZY_VERSION=$V FLOW_OUT=$FLOW FLOW_RENT=$RENTING SKU=$SKU GPUS=$GPUS IMAGE_TOOL=$IMAGE SHOT_S=$SHOT_S FLOW_SPEC=walkflow.spec.js \
  QUIET_MIN=$(param quiet_min 20) HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) \
  CHROME=$(command -v google-chrome) OUT=$OUT ARGV_LOG=$OUT/argv.log NODE_PATH=$PW/node_modules \
  node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/flow.config.js" > "$OUT/flow.log" 2>&1 & node=$!
BG="$BG $node"
# The page orders: its rental joins the trap's list the moment this runner's Calcifer holds it.
while kill -0 "$node" 2> /dev/null; do claim; sleep 20; done
wait "$node"; code=$?
claim
tail -40 "$OUT/flow.log" | cut -c1-300 | sed 's/^/    /' >> "$R"
[ "$code" = 0 ] || { RC=1; say "the flow's browser exited $code"; }
# What the flow saw, in its own words, and what each download is.
jq -r '.failures[]? | "FLOW-FAIL " + .' "$FLOW/flow.json" 2> /dev/null | cut -c1-600 | tee -a "$R"
jq -r '.times[]? | "TIME " + (. | tostring)' "$FLOW/flow.json" 2> /dev/null | cut -c1-900 | tee -a "$R"
jq -r '.said[]? | "SAW " + .' "$FLOW/flow.json" 2> /dev/null | cut -c1-900 | tee -a "$R"
for f in "$FLOW"/downloads/*; do
  [ -f "$f" ] || continue
  say "downloaded ${f##*/}: $(stat -c %s "$f") bytes; $(ffprobe -v error -show_entries stream=codec_type,codec_name,width,height,duration,nb_frames -of compact "$f" 2> /dev/null | tr '\n' ' ')"
done
# Each run's own record: its machine, its GPUs by index, name, UUID and settings (cozy-creator #3820), its times and what it downloaded.
for f in "$FLOW"/runs/*.json; do
  [ -f "$f" ] || continue
  say "record ${f##*/}: $(grep -a '^{' "$f" | tail -1 | jq -c '{name, target, machine, status, reason: (.reason // null), memoized: (.memo.memoized // null),
    gpus: [.measurements.execution.gpus[]? | {gpu, name, uuid, settings, pid}], degree: (.measurements.execution.degree // null),
    weights: (.measurements.weights // null), peak_gib: (((.measurements.memory.peak_allocated_bytes // 0) / 1073741824 * 10 | round) / 10),
    ms: {created: .created_at_ms, accepted: ((.accepted_at_ms // .created_at_ms) - .created_at_ms), started: ((.started_at_ms // .created_at_ms) - .created_at_ms), settled: ((.settled_at_ms // .created_at_ms) - .created_at_ms), execution: (.execution_ms // null)},
    steps: [(.measurements.attribution.steps // {}) | to_entries[] | "\(.key) \(.value.count)"], runtime: (.measurements.software.runtime // null)}' 2> /dev/null | cut -c1-1600)"
done
ids=$({ cat "$OUT/rental" 2> /dev/null; } | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')
timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq --argjson ids "$ids" \
  '[.rentals[] | select(.rental_id as $r | $ids | index($r)) | {rental_id, sku, spend_usd_micros, provider_cost_usd_micros, ran_s}]' > "$FLOW/cost.json" 2> /dev/null ||
  echo '[]' > "$FLOW/cost.json"
say "cost: $(jq -c . "$FLOW/cost.json" | cut -c1-600)"
cozy machine ls > "$FLOW/machine-ls-after.txt" 2>&1 < /dev/null
exit "$RC"
