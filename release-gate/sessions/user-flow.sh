# user-flow (xs-203), the release gate's main suite. Paul, 11-16: "open webui, decrypt project, spawn node, attach tool, rent machine, send
# request to do inference, send another request, rent another machine, send another request, and make sure that the whole thing works and
# is roughly as fast and efficient as it can be. downloading the weights, install the packages on the remote machine, running the
# inference, getting the results back." Then: "a single CI flow that uses the actual frontend in playwright and rents actual servers and
# does actual inference and keeps track of actual download times, download events, and inference events."
# This runner's cozy as a person installs it (pod-session `cozy`: released, a tag, or a dev-build run; a dev build is a dev home, so its
# rentals take the open RC's builds, as Paul's laptop's do). e2e/cli/web/user-flow.spec.js drives Calcifer's page in Chrome: an encrypted
# project unlocked, a plate dropped, H3's Shot and Edit from the Tools palette, machine A ($SKU, one GPU) rented through the page, the Shot
# rendered once its box says it's ready on A or 8 min on (cl-1104), the plate's edit rendered behind it, machine B (sku_b, gpus_b) rented and preferred as it boots, the Shot rendered while B
# boots and again once it's ready (cl-1073's placement on real pods); every take back in the project and shown; both machines ended
# through the page. fixtures/user-flow/judge.py judges what it recorded (works, lean, fast against bench.json, placed
# right, the page tells the truth) into one table per run and per machine: report.md, in the job summary too. Without `rent`, $0: the page
# as far as its quotes for A and B, nothing ordered. Whatever fails, the trap ends what this runner's Calcifer ordered.
# params:
#   sku_b        B's SKU (default: $SKU)
#   gpus_b       B's GPUs (default 2)
#   plate_s      the plate's seconds, the edit's length (default 5)
#   quiet_min    a wait fails once nothing it watches has moved for this long (default 20)
#   dev_home     false: a dev build's rentals stay on their image's release (a red run before the RC's machine fixes); default: a dev
#                build is a dev home, as Paul's laptop is, so its rentals move to the open RC's builds
#   tree         a dev home's packages: a packages_refs checkout's name (`-f packages_refs=rc=<the packages RC>`, tree rc). Its
#                first-party packages run from it, as Paul's laptop's do (`[calcifer] packages`, xs-195). A dev home needs one: the
#                Hub's releases are the last release's, which the RC's Runtime can't run (green 43948824859: minimax-h3 0.13.0rc2 on
#                Runtime 0.47.0.dev2296, every run `TypeError: call() got an unexpected keyword argument 'payload'`)
#   seconds      what the cap must buy of A and B together (default 4800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
WEB=$(cd "$HERE/../../e2e/cli/web" && pwd) || fail "no e2e/cli/web"
FIX=$HERE/fixtures/user-flow
for tool in jq node npm google-chrome ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
SKU_B=$(param sku_b "$SKU") GPUS_B=$(param gpus_b 2) PLATE_S=$(param plate_s 5) DEV_HOME=$(param dev_home true) TREE=$(param tree '')
[[ $GPUS_B =~ ^[1-8]$ && $PLATE_S =~ ^[0-9]+$ && $PLATE_S -ge 1 && $PLATE_S -le 15 ]] || fail "gpus_b $GPUS_B, plate_s $PLATE_S"
FLOW=$OUT/flow
mkdir -p "$FLOW"
# Encryption's filesystem driver, as a person's computer has it: FUSE 3 beside libfuse.so.2 (the bundled provider's, and a release's).
ROOT=$(cd "$HERE/../.." && pwd)
bash "$ROOT/e2e/cli/install-vault-driver.sh" fuse3 > "$OUT/fuse.log" 2>&1 || fail "no FUSE: $(tail -c 300 "$OUT/fuse.log")"
# And its provider: a release bundles securefs and puts it in the tools folder on first use; a dev build bundles none, so the pinned
# one goes where a release puts it (crates/project-mount: the tools folder's copy before PATH).
SECUREFS=$HOME/.cozy/tools/securefs/$(jq -r .version "$ROOT/crates/project-mount/helper/manifest.json")
if [ ! -x "$SECUREFS/securefs" ]; then
  archive=$(python3 -I "$ROOT/scripts/prepare-securefs.py" --target x86_64-unknown-linux-gnu --out-dir "$RUNNER_TEMP/securefs") || fail "no securefs"
  mkdir -p "$SECUREFS" && chmod 700 "$SECUREFS" &&
    python3 -I -c 'import sys, zipfile; open(sys.argv[2], "wb").write(zipfile.ZipFile(sys.argv[1]).read("securefs"))' "$archive" "$SECUREFS/securefs" &&
    chmod 755 "$SECUREFS/securefs" || fail "securefs not put in $SECUREFS"
fi
V=$(cozy --version | awk '{print $2}')
SOURCE=
if [[ $V == *-dev* && $DEV_HOME != false ]]; then
  cfg=$HOME/.cozy/config.toml
  grep -qxE 'dev *= *true' "$cfg" 2> /dev/null ||
    { mkdir -p "${cfg%/*}" && { echo 'dev = true'; [ ! -f "$cfg" ] || grep -v '^dev *=' "$cfg"; } > "$cfg.new" && mv "$cfg.new" "$cfg"; } || fail "dev = true not written"
  # Its packages, from the checkout, with the Triton sets a release ships (built before anything is rented).
  [ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] ||
    fail "a dev home runs first-party packages from a checkout: dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
  SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
  kernels "$SOURCE/minimax-h3"
fi
# Each project's walk says where it placed each take, every machine's quote beside it (tests only, `calcifer.diagnostics`): the judge's
# record of the placement, kept in the project's own storage.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; if (source != "") print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" 2> /dev/null && mv "$cfg.next" "$cfg" || fail "calcifer.diagnostics not written"
# Calcifer started before the driver, the dev home and its diagnostics: started again, as after an install.
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
# What this runner's Calcifer ordered is this session's: the page orders it.
trap 'code=$?; claim; (exit "$code"); finish' EXIT

nice -n 19 ffmpeg -v error -y -f lavfi -i "testsrc2=size=1344x768:rate=24:duration=$PLATE_S" -c:v libx264 -pix_fmt yuv420p "$FLOW/plate.mp4" ||
  fail "no plate"
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
# The cap must buy A and B: checked here whether or not this rents. Renting, the guard watches what this runner's Calcifer ordered.
RENTING=$RENT
RENT=true quote "$(param seconds 4800)" "$SKU" "$SKU_B:$GPUS_B"
GUARD_ALSO=own
[ "$RENTING" != true ] || guard
say "the flow on cozy $V$(grep -qxE 'dev *= *true' "$HOME/.cozy/config.toml" 2> /dev/null && echo ', a dev home')$([ -z "$SOURCE" ] || echo ", packages from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null), minimax-h3 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/minimax-h3/pyproject.toml" | head -1)"): A $SKU x1, B $SKU_B x$GPUS_B, a ${PLATE_S} s plate$([ "$RENTING" = true ] || echo '; pre-flight: the page up to its quotes, nothing ordered')"

COZY_VERSION=$V FLOW_OUT=$FLOW FLOW_RENT=$RENTING SKU=$SKU SKU_B=$SKU_B GPUS_B=$GPUS_B PLATE=$FLOW/plate.mp4 PLATE_S=$PLATE_S \
  QUIET_MIN=$(param quiet_min 20) HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) \
  CHROME=$(command -v google-chrome) OUT=$OUT ARGV_LOG=$OUT/argv.log NODE_PATH=$PW/node_modules \
  node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/flow.config.js" > "$OUT/flow.log" 2>&1 & node=$!
BG="$BG $node"
# The page orders: each rental joins the trap's list the moment this runner's Calcifer holds it.
while kill -0 "$node" 2> /dev/null; do claim; sleep 30; done
wait "$node"; code=$?
claim
tail -30 "$OUT/flow.log" | cut -c1-300 | sed 's/^/    /' >> "$R"
node "$WEB/report.js" "$OUT/web/report.json" user-flow > "$OUT/report.txt" 2>&1
cat "$OUT/report.txt" >> "$R"
! grep -q '^FAILED' "$OUT/report.txt" || RC=1
[ "$code" = 0 ] || { RC=1; say "the flow's browser exited $code"; }

# What the Hub bills for each rental, read once the page ended them.
ids=$({ cat "$OUT/rental" 2> /dev/null; } | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')
timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq --argjson ids "$ids" \
  '[.rentals[] | select(.rental_id as $r | $ids | index($r)) | {rental_id, sku, spend_usd_micros, provider_cost_usd_micros, ran_s}]' > "$FLOW/cost.json" 2> /dev/null ||
  echo '[]' > "$FLOW/cost.json"
python3 -I "$FIX/judge.py" "$FLOW" "$FIX/bench.json" "$HERE/.." > "$FLOW/report.md" 2> "$FLOW/judge.err" || RC=1
[ -s "$FLOW/report.md" ] || { RC=1; say "the judge said nothing: $(tail -c 500 "$FLOW/judge.err")"; }
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || cat "$FLOW/report.md" >> "$GITHUB_STEP_SUMMARY"
grep -E '^(FAIL|NOTE) ' "$FLOW/verdict.txt" 2> /dev/null | sed 's/^/  /' | tee -a "$R"
say "user-flow: $(grep -c '^PASS' "$FLOW/verdict.txt" 2> /dev/null || echo 0) passed, $(grep -c '^FAIL' "$FLOW/verdict.txt" 2> /dev/null || echo 0) failed (report.md)"
exit "$RC"
