# llm-engines (xs-212): captioning on Cozy's own LLM path against an inference engine on the same rental of $SKU.
# Paul, 11-17: "using Gollama / sglang / vllm or whatever inference engine is fastest (probably faster than running them with
# basic-pytorch as the backend). Our first real use-case for them will be captioning."
# A dev home on this runner's RC cozy, the packages from the packages_refs checkout `tree`, as llm-tools does. The workload
# (fixtures/llm-engines): 32 photos at 1280x720 (picsum ids, sha256 checked) and 3 ten-second 720p clips, the package's own
# `caption` question, greedy.
#   cozy    `<package>/caption` by `cozy run --on` the rental: one cold call (install, download, build), then the 32 photos one
#           at a time, 8 and 32 in flight (separate runs, as boxes on a page are), as `ask` batches of 16 prompts, and the clips.
#           Each run's wall time from this runner and its record's execution time; nvidia-smi on the pod every second.
#   engine  the same workload through an engine's OpenAI-compatible server on the pod over ssh (fixtures/llm-engines/bench.py):
#           load time, TTFT, the 512-token decode, 1/8/32 in flight, peak VRAM. Weights come from Hugging Face on the pod.
# Each answer is kept (answers.json): the engine's against the cozy path's, word for word.
# Without `rent`, $0: the fixtures fetched and checked, the quote.
# params:
#   tree     the packages_refs checkout (required)
#   package  qwen3.5 (default) or qwen3.6; model: a `model` override (default: the package's)
#   only     some of cozy, engine (default both)
#   engine   {"install": [install.sh words: engines and models], "configs": [plan.sh configs, run in order]}
#   seconds  what the cap must buy (default 7200)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/llm-engines
TREE=$(param tree '') PACKAGE=$(param package qwen3.5) MODEL=$(param model '') ONLY=$(param only cozy,engine) ENGINE=$(param engine '{}')
part() { [[ ",$ONLY," == *",$1,"* ]]; }
for tool in jq python3 curl ffprobe; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/$PACKAGE/pyproject.toml" ] || fail "no $PACKAGE in packages_refs checkout '$TREE'"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/$PACKAGE"
# The workload, on this runner and (engine) on the pod: the same bytes.
DATA=$OUT/data
mkdir -p "$DATA/images" "$DATA/videos"
while read -r sum f; do
  id=${f#p}; id=$((10#${id%.jpg}))
  curl -sfL --retry 3 -o "$DATA/images/$f" "https://picsum.photos/id/$id/1280/720" || fail "fetch $f"
done < "$FIX/images.sha256"
while read -r sum f; do
  curl -sfL --retry 3 -o "$DATA/videos/$f" "$(awk -v f="$f" '$2 == f {print $1}' "$FIX/videos.url")" || fail "fetch $f"
done < "$FIX/videos.sha256"
(cd "$DATA/images" && sha256sum -c --quiet "$FIX/images.sha256") || fail "the photos changed upstream"
(cd "$DATA/videos" && sha256sum -c --quiet "$FIX/videos.sha256") || fail "the clips changed upstream"
mapfile -t IMAGES < <(ls "$DATA"/images/*.jpg)
mapfile -t VIDEOS < <(ls "$DATA"/videos/*.mp4)
[ "${#IMAGES[@]}" = 32 ] && [ "${#VIDEOS[@]}" = 3 ] || fail "workload: ${#IMAGES[@]} photos, ${#VIDEOS[@]} clips"
say "workload: 32 photos, 3 clips, sha256 checked"
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
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
RENTING=$RENT
RENT=true quote "$(param seconds 7200)"
[ "$RENTING" = true ] || { say "pre-flight: the workload and the quote; nothing rented"; exit "$RC"; }
GUARD_ALSO=own guard
order
ready
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
step hold machine update "$NAME" --runtime "$(jq -r '.status.runtime' "$OUT/show-rc.json")" -y
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
# keepalive: ssh-only work is no user command (15-minute idle release)
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
SMI=$OUT/smi.csv
there "nvidia-smi --query-gpu=memory.used,utilization.gpu --format=csv,noheader,nounits -l 1 | while IFS= read -r l; do echo \"\$(date +%s),\$l\"; done" 14400 \
  < /dev/null > "$SMI" 2> "$OUT/smi.err" & BG="$BG $!"
mark() { echo "$(date +%s) $1" >> "$OUT/phases.txt"; say "phase $1"; }

M=(); [ -z "$MODEL" ] || M=(--model "model=$MODEL")
# one <label> <file> <function> <inputs…>: one run on the rental, its wall time and record (runs/<label>.json).
one() {
  local label=$1 t0 t1 n; shift
  t0=$(date +%s.%N)
  (cd "$RUN_PROJECT" && cozy run "$SOURCE/$PACKAGE/$1" "${@:2}" "${M[@]}" --on "$NAME") > "$OUT/runs/$label.txt" 2>&1
  t1=$(date +%s.%N)
  n=$(runno "runs/$label")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/runs/$label.json" 2> /dev/null
  echo "{\"label\":\"$label\",\"run\":\"$n\",\"wall_s\":$(python3 -c "print(round($t1-$t0,3))")}" >> "$OUT/runs.jsonl"
}
# flight <phase> <in flight>: the 32 photos, as separate caption runs, that many at once.
flight() {
  local phase=$1 most=$2 i=0 t0 t1
  t0=$(date +%s.%N); mark "$phase-start"
  local pids=() live p
  for f in "${IMAGES[@]}"; do
    while :; do
      live=(); for p in "${pids[@]}"; do kill -0 "$p" 2> /dev/null && live+=("$p"); done; pids=("${live[@]}")
      [ "${#pids[@]}" -lt "$most" ] && break
      sleep 0.2
    done
    one "$phase-$(basename "$f" .jpg)" caption --file "images=$f" & pids+=($!)
    i=$((i + 1))
  done
  for p in "${pids[@]}"; do wait "$p"; done
  t1=$(date +%s.%N); mark "$phase-end"
  say "$phase: 32 captions, $most in flight: $(python3 -c "print(round($t1-$t0,1))") s"
}
if part cozy; then
  mkdir -p "$OUT/runs"
  mark cold-start
  one cold caption --file "images=${IMAGES[0]}"
  mark cold-end
  say "cold: $(tail -1 "$OUT/runs.jsonl")"
  flight c1 1
  flight c8 8
  flight c32 32
  mark b16-start
  for half in 0 16; do
    files=(); for k in $(seq 0 15); do files+=(--file "prompts.$k.images=${IMAGES[$((half + k))]}"); done
    prompts=$(jq -cn '[range(16) | {text: "Say what this shows, in one or two plain sentences, for someone who can'"'"'t see it: who or what it is and how it looks. Say only what you can see."}]')
    one "b16-$half" ask "prompts:=$prompts" "${files[@]}"
  done
  mark b16-end
  mark videos-start
  for f in "${VIDEOS[@]}"; do one "videos-$(basename "$f" .mp4)" caption --file "videos=$f"; done
  mark videos-end
fi
if part engine && [ "$(jq -r '.configs // [] | length' <<<"$ENGINE")" != 0 ]; then
  there "mkdir -p /w/data /w/out && export DEBIAN_FRONTEND=noninteractive && (command -v curl && command -v file) > /dev/null || (apt-get update -qq && apt-get install -y -qq curl file) > /dev/null 2>&1" 600 < /dev/null || fail "pod: /w and curl"
  for f in bench.py run.sh setup.sh install.sh plan.sh; do cozy machine ssh "$NAME" -- "cat > /w/$f && chmod +x /w/$f" < "$FIX/$f" || fail "pod: $f"; done
  mark engine-setup-start
  there "bash /w/setup.sh" 1800 < /dev/null > "$OUT/engine-setup.txt" 2>&1 || fail "pod setup: $(tail -c 300 "$OUT/engine-setup.txt")"
  there "cd /w/data/images && sha256sum -c --quiet -" 60 < "$FIX/images.sha256" || fail "the pod's photos differ"
  there "bash /w/install.sh $(jq -r '.install | join(" ")' <<<"$ENGINE")" 3600 < /dev/null > "$OUT/engine-install.txt" 2>&1
  there "cat /w/out/install.jsonl" 60 < /dev/null | tee -a "$R"
  mark engine-setup-end
  for config in $(jq -r '.configs[]' <<<"$ENGINE"); do
    mark "engine-$config-start"
    there "bash /w/plan.sh $config" 5400 < /dev/null > "$OUT/engine-$config.txt" 2>&1
    mark "engine-$config-end"
    there "cd /w/out && tar cz $config.*" 120 < /dev/null > "$OUT/engine-$config.tgz" && tar xzf "$OUT/engine-$config.tgz" -C "$OUT"
    grep -a '^{' "$OUT/engine-$config.txt" | cut -c1-300 | sed "s/^/  $config: /" | tee -a "$R"
  done
fi
python3 -I "$FIX/summary.py" "$OUT" > "$OUT/summary.md" 2>&1 || RC=1
cat "$OUT/summary.md" | tee -a "$R"
{ echo "## llm-engines on $NAME ($SKU)"; cat "$OUT/summary.md"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
