# llm-stream (xs-209): an LLM's words as they come, on one rental of $SKU (a 48 GB card), the page in Chrome on this runner.
# Paul, 11-17: "we can type text into boxes and get a reply from it (ideally streaming the results), and also we can get captions for
# images (ideally streaming)". A dev home on this session's cozy (pod-session `cozy`: a dev-build run of the change), its first-party
# packages from the packages_refs checkout `tree`. The rental runs this cozy's TensorD and the change's Runtime (`runtime`: a cozy-runtime
# dev-build run id, whose wheel carries the kernels), held there for the session. `llm-stream: a real model on a GPU`
# (e2e/cli/web/llm-stream.spec.js) renders a Text box of qwen3's Write on it, cold then warm, then a question thinking at each effort: each render's time from its ask to
# its first word on the box, and the words a second the box shows against the tokens a second its run counts (numbers.json).
# Without `rent`, $0: the quote, and the checks before it.
# params:
#   tree     the packages_refs checkout qwen3 runs from (required)
#   runtime  the cozy-runtime dev-build run id of the Runtime the rental runs (required)
#   model    the checkpoint its `model` slot reads (default: the package's), e.g. heretic-org/qwen3.8-27b-heretic-ara@sha256:<int8>
#   seconds  what the cap must buy (default 2700)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
for tool in jq node npm google-chrome python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
TREE=$(param tree '') SRC=$(param runtime '') MODEL=$(param model '')
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] ||
  fail "qwen3 runs from a checkout: dispatch with -f packages_refs=<name>=<its branch> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
say "runtime: ${W##*/}"
kernels "$SOURCE/qwen3"
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
quote "$(param seconds 2700)"
GUARD_ALSO=own guard
rent
# The dev home's follower moves it to the RC's builds once it's ready; then it's moved to this change's and held there (an update naming its
# Runtime), so nothing moves it between renders.
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "the pod's TensorD and Runtime"
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-held.json" 2> /dev/null
  [ "$(jq -r '.state // empty' "$OUT/show-held.json")" = ready ] && break
  sleep 5
done
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · held \(.held // "no") · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-held.json")"

say "page: qwen3/write on $NAME, model ${MODEL:-its default}"
env STREAM_MACHINE="$NAME" STREAM_TOOL=qwen3/write ${MODEL:+STREAM_MODEL="$MODEL"} HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" \
  COZY="$(command -v cozy)" CHROME="$(command -v google-chrome)" OUT="$OUT" ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" \
  node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" -g 'llm-stream: a real model on a GPU' > "$OUT/page.log" 2>&1
code=$?
cp "$OUT/web/report.json" "$OUT/web/report-page.json" 2> /dev/null
node "$WEB/report.js" "$OUT/web/report-page.json" llm-stream > "$OUT/page.report" 2>&1
sed 's/^/    /' "$OUT/page.report" >> "$R"
[ "$code" = 0 ] || { say "FAIL page: exit $code ($(grep -m1 -E '^FAILED' "$OUT/page.report" | cut -c1-300))"; RC=1; }
N=$OUT/web/llm-stream-gpu/numbers.json
jq -r '.renders[] | "  render \(.render): run \(.run) on \(.gpu // "?"), first word on screen \(.first_word_ms) ms, \(.words_per_s // 0 | . * 10 | round / 10) words/s shown, \(.tokens_per_s // 0 | . * 10 | round / 10) tokens/s counted, \(.words) words, \(.tokens) tokens, executed \(.executed_ms) ms, landed \(.landed_ms) ms; stages \(.stages | join(", "))"' "$N" 2> /dev/null | tee -a "$R"
cp "$OUT"/web/llm-stream-gpu/*.png "$OUT/" 2> /dev/null
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## llm-stream on $NAME ($SKU)"; jq -r '.renders[] | "- render \(.render): first word \(.first_word_ms) ms, \(.words_per_s) words/s shown, \(.tokens_per_s) tokens/s"' "$N" 2> /dev/null; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
