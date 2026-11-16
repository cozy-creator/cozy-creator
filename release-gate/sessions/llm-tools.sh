# llm-tools (xs-204, cl-1100): LLM tools on the canvas and a machine's live GPU use, on one rental of $SKU with `gpus` GPUs.
# xs-204, Paul 11-16: "ideally I want LLM tools as well. Like I can create a text box, and then type text into it. Then I can attach an
# LLM and it can generate more text. Or I can attach an LLM (ideally VLLM) to an image node, or a video node, and ask it to produce a
# caption, and it'll produce text".
# cl-1100, Paul 11-16: "4. a list of its GPUs and GPU utilization. This is what Runpod does. That way we can see how hard its GPUs are
# working. Making sure its GPUs are working, and making sure disc space isn't too full, is probably the top thing we need to get right".
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), its first-party packages from the
# packages_refs checkout `tree`, as Paul's laptop runs them (xs-195). The rental moves to the RC's builds (its TensorD's GET /v1/gpus, its
# Runtime's vlm.TextBox) before anything runs on it.
#   cli    Qwen3.5's and Qwen3.6's `write` (their defaults, 9B and 35B-A3B) by `cozy run --on` the rental, from the checkout: each asked
#          about a photo (teapot.jpg) and a video (clip-1s-360p.mp4, a lighthouse at dusk) by mention, boxes named `photo` and `clip`
#          so their names say nothing; each answer must say what it shows. Photos first (cold: install, download), then the videos.
#   gpus   cl-1100: both write a long story at once while the drawer (`machine-details: GPU use`, GPU_MACHINE) is read twice a second
#          and the pod's nvidia-smi once a second: each GPU's busy % and memory within 10 points and 200 MB, and the drawer showing the
#          reading it heard (fixtures/llm-tools/gpu_use.py); the GPUs section at 1440×900 and 1920×1080, paper and dark.
#   page   xs-204 in Chrome (`llm: real models on a GPU`, gpu.config.js), the project's renders on the rental: Qwen3.6 on a Text box
#          rendered twice, two versions of words; let go on the photo and on the video, a new box beside it, "write a caption"
#          rendered: its run carries the file and its answer describes it.
# Without `rent`, $0: the quote, then both page cells on this runner's own machine, the LLM the local fixture (e2e/cli/project/llm.py),
# so the hands are proven before anything is rented. Each run's numbers and answers in summary.md and the job summary.
# params:
#   tree     the packages_refs checkout the packages run from (required)
#   gpus     the rental's GPUs (default 2)
#   busy_s   seconds the drawer is read during the stories (default 60)
#   seconds  what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web FIX=$HERE/fixtures/llm-tools
PICTURE=$HERE/fixtures/ltx-images/teapot.jpg VIDEO=$ROOT/release-gate/suites/bench/fixtures/clip-1s-360p.mp4
SEES_PICTURE='tea ?pot|kettle' SEES_VIDEO='lighthouse'
for tool in jq node npm google-chrome python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$PICTURE" ] && [ -f "$VIDEO" ] || fail "no $PICTURE or $VIDEO (pod-session.yaml's sparse checkout)"
TREE=$(param tree '') GPUS=$(param gpus 2) BUSY_S=$(param busy_s 60)
[[ $GPUS =~ ^[1-8]$ && $BUSY_S =~ ^[0-9]+$ ]] || fail "gpus $GPUS, busy_s $BUSY_S"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3.5/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/qwen3.6/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/qwen3.5"
kernels "$SOURCE/qwen3.6"
# A dev home, its first-party packages from the checkout.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg"; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
# ssh for nvidia-smi on the pod
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh; [ "$GPUS" = 1 ] || RENT_ARGS="--ssh --gpus $GPUS"
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"

# pw <label> <cell> <test title> [VAR=value…]: one GPU branch of a web cell (gpu.config.js), its report as web.sh says it.
pw() {
  local label=$1 cell=$2 title=$3 code; shift 3
  say "page $label: $title"
  env "$@" HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY="$(command -v cozy)" CHROME="$(command -v google-chrome)" OUT="$OUT" \
    ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" \
    -g "$title" > "$OUT/$label.log" 2>&1
  code=$?
  cp "$OUT/web/report.json" "$OUT/web/report-$label.json" 2> /dev/null
  node "$WEB/report.js" "$OUT/web/report-$label.json" "$cell" > "$OUT/$label.report" 2>&1
  sed 's/^/    /' "$OUT/$label.report" >> "$R"
  [ "$code" = 0 ] || { say "FAIL page $label: exit $code ($(grep -m1 -E '^FAILED' "$OUT/$label.report" | cut -c1-300))"; RC=1; }
  return "$code"
}

RENTING=$RENT
RENT=true quote "$(param seconds 3600)"
if [ "$RENTING" != true ]; then
  say "pre-flight: nothing rented; the page cells on this runner's machine, the LLM the local fixture"
  pw dry-gpus machine-details 'machine-details: GPU use' GPU_MACHINE=local
  pw dry-llm llm 'llm: real models on a GPU' LLM_MACHINE=local LLM_TOOL="$ROOT/e2e/cli/project/llm.py/ask" LLM_PICTURE="$PICTURE" \
    LLM_VIDEO="$VIDEO" LLM_SEES_PICTURE='shown: an image [0-9]+px wide' LLM_SEES_VIDEO='shown: a video'
  exit "$RC"
fi
GUARD_ALSO=own guard
order
ready
# The dev home moves it to the RC's builds once it's ready, before its first run.
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"

# cli: each package asked about the photo and the video, by mention.
run_page import "$PICTURE" --name photo > "$OUT/import-photo.json" 2>&1 || fail "import the photo: $(tail -c 300 "$OUT/import-photo.json")"
run_page import "$VIDEO" --name clip > "$OUT/import-clip.json" 2>&1 || fail "import the video: $(tail -c 300 "$OUT/import-clip.json")"
for _ in $(seq 100); do
  photo=$(run_page asset photo | jq -r '.sha256 // empty') clip=$(run_page asset clip | jq -r '.sha256 // empty')
  [ -n "$photo" ] && [ -n "$clip" ] && break; sleep 0.2
done
[ -n "$photo" ] && [ -n "$clip" ] || fail "the imports hold no files: $photo $clip"
ask() { jq -cn --arg k "$1" --arg t "$2" --arg f "sha256:${3#sha256:}" --arg q "$4" '{text: $q, references: [{key: $k, type: $t, file: $f}]}'; }
ON_PHOTO=$(ask photo image "$photo" 'What is in <photo>? Answer in two sentences.')
ON_CLIP=$(ask clip video "$clip" 'What does <clip> show? Answer in two sentences.')
# write <cell> <package> <inputs…>: its `write` from the checkout on the rental, then its whole record (<cell>.json).
write() {
  local cell=$1 package=$2 n code; shift 2
  step "$cell" run "$SOURCE/$package/write" "$@" --on "$NAME"; code=$?
  n=$(runno "$cell")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$cell.json" 2> "$OUT/$cell.err"
  say "$cell: run ${n:-none} $(jq -r '.status // "unrecorded"' "$OUT/$cell.json" 2> /dev/null): $(jq -r '.result.text // empty' "$OUT/$cell.json" 2> /dev/null | tr '\n' ' ' | cut -c1-400)"
  return "$code"
}
write q35-photo qwen3.5 "instructions:=$ON_PHOTO" max_tokens:=200
write q36-photo qwen3.6 "instructions:=$ON_PHOTO" max_tokens:=200
write q35-clip qwen3.5 "instructions:=$ON_CLIP" max_tokens:=200
write q36-clip qwen3.6 "instructions:=$ON_CLIP" max_tokens:=200

# gpus: the pod's nvidia-smi once a second, beside the drawer, while both write at once.
SMI=$OUT/smi.csv fields=index,utilization.gpu,memory.used,memory.reserved
cozy machine ssh "$NAME" "nvidia-smi --query-gpu=$fields --format=csv,noheader,nounits" < /dev/null > "$OUT/smi-probe.txt" 2>&1 ||
  fields=index,utilization.gpu,memory.used
say "gpus: nvidia-smi --query-gpu=$fields each second on $NAME: $(grep -v '^Warning' "$OUT/smi-probe.txt" | paste -sd';')"
cozy machine ssh "$NAME" "timeout 1200 nvidia-smi --query-gpu=$fields --format=csv,noheader,nounits -l 1 | while IFS= read -r l; do echo \"\$(date +%s%3N),\$l\"; done" \
  < /dev/null > "$SMI" 2> "$OUT/smi.err" & smi=$!
BG="$BG $smi"
STORY='Write a very long story, at least 3000 words in twenty numbered chapters, about a lighthouse keeper through one winter of storms.'
write q35-story qwen3.5 "instructions=$STORY" max_tokens:=6000 & s35=$!
write q36-story qwen3.6 "instructions=$STORY" max_tokens:=6000 & s36=$!
BG="$BG $s35 $s36"
for _ in $(seq 150); do tail -4 "$SMI" 2> /dev/null | awk -F, '$3 + 0 >= 30 { f = 1 } END { exit !f }' && break; sleep 2; done
say "gpus: busy now: $(tail -2 "$SMI" | paste -sd';')"
pw gpus machine-details 'machine-details: GPU use' GPU_MACHINE="$NAME" GPU_LOG="$OUT/drawer.jsonl" GPU_LOG_S="$BUSY_S"
wait "$s35"; wait "$s36"
kill "$smi" 2> /dev/null
python3 -I "$FIX/gpu_use.py" "$SMI" "$OUT/drawer.jsonl" "$OUT/drawer.jsonl.heard" > "$OUT/gpu-use.md" 2>&1 || RC=1
grep -E '^(PASS|FAIL) ' "$OUT/gpu-use.md" | sed 's/^/  gpus: /' | tee -a "$R"
find "$OUT/web/results" -name 'gpu-use-*.png' -exec cp {} "$OUT/" \; 2> /dev/null

# page: Qwen3.6 on the canvas, its renders on the rental.
pw llm llm 'llm: real models on a GPU' LLM_MACHINE="$NAME" LLM_TOOL=qwen3.6/write LLM_PICTURE="$PICTURE" LLM_VIDEO="$VIDEO" \
  LLM_SEES_PICTURE="$SEES_PICTURE" LLM_SEES_VIDEO="$SEES_VIDEO"
jq -r '.boxes | to_entries[] | .key as $box | .value[] | "  page: \($box) v\(.version) run \(.run) on \(.machine) GPU \(.gpu // "?"), \(.execution_ms // "?") ms\(if .carries == null then "" else ", carries its \(.kind): \(.carries)" end): \(.text | gsub("\n"; " "))"' \
  "$OUT/web/llm-gpu/answers.json" 2> /dev/null | tee -a "$R"

cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
python3 -I "$FIX/summary.py" "$OUT" q35-photo="$SEES_PICTURE" q36-photo="$SEES_PICTURE" q35-clip="$SEES_VIDEO" q36-clip="$SEES_VIDEO" \
  q35-story q36-story > "$OUT/summary.md" 2>&1 || RC=1
grep -E '^(PASS|FAIL) ' "$OUT/summary.md" | sed 's/^/  cli: /' | tee -a "$R"
{ echo "## llm-tools on $NAME ($SKU x$GPUS)"; cat "$OUT/summary.md"; echo; echo "### GPU use against nvidia-smi"; cat "$OUT/gpu-use.md"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
