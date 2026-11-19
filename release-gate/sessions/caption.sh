# caption (se-151): qwen3's Caption on real pictures, its words Qwen Image 2.1 Upscale's prompt, on one rental of $SKU.
# Paul, 11-18: "add the caption tool to an image, (2) run it, it produces a caption, (3) then add the upscale tool, and use that caption as one
# of its inputs. That way they're composable". As a person builds it on the canvas, through the CLI's verbs for the same routes: each
# picture (a 256 px JPEG of fixtures/ltx-images/teapot.jpg and torchvision's dog1.jpg, as small as se-147's) a node; a Caption node
# reading it (`<teapot>`), rendered on the rental on its default model; then Upscale on the picture, its prompt `<cap_teapot>`, rendered there
# too. What its run was sent (`request.input.prompt.text`) and what it encoded (`result.prompt`) carry the caption's words; each caption,
# each run's record and each upscaled picture are kept in $OUT for a person to read and open.
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), its first-party packages from the
# packages_refs checkout `tree`, as llm-tools. The rental moves to the RC's builds and is held there. Without `rent`, $0: the quote only.
# params:
#   tree     the packages_refs checkout the packages run from (required)
#   seconds  what the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
for tool in jq curl ffmpeg; do command -v "$tool" > /dev/null || fail "no $tool"; done
TREE=$(param tree '')
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<its branch> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
kernels "$SOURCE/qwen3"
kernels "$SOURCE/qwen-image-2"
# The pictures, small and compressed: what an upscale is for.
mkdir -p "$OUT/inputs"
curl -fsSL -o "$OUT/inputs/dog-full.jpg" https://raw.githubusercontent.com/pytorch/vision/v0.24.0/gallery/assets/dog1.jpg || fail "dog1.jpg not downloaded"
for one in "teapot:$HERE/fixtures/ltx-images/teapot.jpg" "dog:$OUT/inputs/dog-full.jpg"; do
  ffmpeg -v error -y -i "${one#*:}" -vf scale=256:-2 -q:v 12 "$OUT/inputs/${one%%:*}.jpg" || fail "${one%%:*}: not made small"
done
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
# The nodes, written before anything is rented: what the $0 pre-flight checks.
PICTURES=(teapot dog)
# in_run <cozy args…>: a verb in the session's project, as a person runs it there.
in_run() { (cd "$RUN_PROJECT" && cozy "$@"); }
{ for p in "${PICTURES[@]}"; do printf '%s:\n  kind: image\n  takes:\n    - file: %s\n' "$p" "$OUT/inputs/$p.jpg"; done; } > "$OUT/pictures.yaml"
in_run apply -f "$OUT/pictures.yaml" > "$OUT/apply-pictures.txt" 2>&1 || fail "the pictures: $(tail -c 300 "$OUT/apply-pictures.txt")"
{ for p in "${PICTURES[@]}"; do printf 'cap_%s:\n  kind: text\n  qwen3/caption:\n    instructions: "<%s>"\n' "$p" "$p"; done; } > "$OUT/captions.yaml"
in_run apply -f "$OUT/captions.yaml" > "$OUT/apply-captions.txt" 2>&1 || fail "the Caption nodes: $(tail -c 300 "$OUT/apply-captions.txt")"
# Upscale on each picture, as a person puts it there: its take in `image`, its prompt the caption's mention. A node already there
# merges against the rev it was read at.
rev=$(in_run --json get "${PICTURES[@]}" 2> "$OUT/get-pictures.err" | jq -r '.rev // empty')
[ -n "$rev" ] || fail "the pictures' rev: $(tail -c 300 "$OUT/get-pictures.err")"
{ for p in "${PICTURES[@]}"; do printf '%s:\n  qwen-image-2/upscale:\n    image: "<self.1>"\n    prompt: "<cap_%s>"\n  rev: "%s"\n' "$p" "$p" "$rev"; done; } > "$OUT/upscales.yaml"
in_run apply -f "$OUT/upscales.yaml" > "$OUT/apply-upscales.txt" 2>&1 || fail "Upscale on the pictures: $(tail -c 300 "$OUT/apply-upscales.txt")"
in_run --json get "${PICTURES[@]}" "${PICTURES[@]/#/cap_}" > "$OUT/nodes.json" 2>&1 || fail "the nodes: $(tail -c 300 "$OUT/nodes.json")"
for p in "${PICTURES[@]}"; do
  jq -e --arg p "$p" '.nodes[$p]["qwen-image-2/upscale"] | .image == "<self.1>" and .prompt == "<cap_\($p)>"' "$OUT/nodes.json" > /dev/null &&
    jq -e --arg p "$p" '.nodes["cap_\($p)"]["qwen3/caption"].instructions == "<\($p)>"' "$OUT/nodes.json" > /dev/null ||
    fail "$p's nodes aren't as written: $(jq -c --arg p "$p" '{($p): .nodes[$p], cap: .nodes["cap_\($p)"]}' "$OUT/nodes.json" | head -c 600)"
done
say "nodes: ${PICTURES[*]}, each with Upscale reading <cap_…>, and its Caption node"
quote "$(param seconds 2400)"
GUARD_ALSO=own guard
rent
# The dev home moves it to the RC's builds once it's ready; then it's held there, so nothing moves it between renders (llm-tools).
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
step hold machine update "$NAME" --runtime "$(jq -r '.status.runtime' "$OUT/show-rc.json")" -y
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"

cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
step captions render "${PICTURES[@]/#/cap_}" --on "$NAME" || fail "the captions: $(tail -c 300 "$OUT/captions.txt")"
# records <target> <file>: every run of <target> in the session's project, each whole (`run watch --full`), as a JSON list.
records() {
  local n
  in_run --json run ls --limit 50 > "$OUT/runs-list.json" 2> "$OUT/runs-list.err"
  for n in $(jq -r --arg t "$1" '.runs[]? | select(.kind == "call" and (.target | test($t + "(@|$)"))) | .number' "$OUT/runs-list.json"); do
    in_run --json --full run watch "$n" --once 2> /dev/null
  done | jq -s . > "$2"
}
records /qwen3/caption "$OUT/caption-runs.json"
for p in "${PICTURES[@]}"; do
  jq -r --arg m "<$p>" '[.[] | select(.request.input.instructions.text == $m)] | last | .result.text // empty' "$OUT/caption-runs.json" > "$OUT/cap_$p.txt"
  [ -s "$OUT/cap_$p.txt" ] || { say "FAIL cap_$p: no caption run read <$p>: $(head -c 300 "$OUT/runs-list.json")"; RC=1; continue; }
  say "cap_$p: $(wc -w < "$OUT/cap_$p.txt") words: $(tr '\n' ' ' < "$OUT/cap_$p.txt")"
done

# Then Upscale on each picture, the caption's words its prompt.
step upscales render "${PICTURES[@]}" --on "$NAME" || fail "the upscales: $(tail -c 300 "$OUT/upscales.txt")"
records /qwen-image-2/upscale "$OUT/upscale-runs.json"
for p in "${PICTURES[@]}"; do
  words=$(cat "$OUT/cap_$p.txt" 2> /dev/null)
  [ -n "$words" ] || continue
  # Its run: the one sent this caption's words as its prompt; what it encoded follows the restoration request.
  jq --arg w "$words" '[.[] | select(.request.input.prompt.text == $w)] | last // empty' "$OUT/upscale-runs.json" > "$OUT/$p.json"
  [ -s "$OUT/$p.json" ] || { say "FAIL $p: no Upscale run was sent cap_$p's words as its prompt"; RC=1; continue; }
  encoded=$(jq -r '.result.prompt // empty' "$OUT/$p.json")
  [[ $encoded == *"$words"* ]] || { say "FAIL $p: Upscale's encoded prompt left the caption out: $(head -c 400 <<< "$encoded")"; RC=1; }
  in_run --json get "$p" --takes > "$OUT/$p-node.json" 2>&1
  file=$(jq -r --arg n "$p" '[.nodes[$n].takes[]? | select(.state == "done")] | max_by(.number) | .file // empty' "$OUT/$p-node.json")
  [ -f "$file" ] && cp "$file" "$OUT/$p-upscaled.${file##*.}"
  say "$p: Upscale run $(jq -r '.name // .number' "$OUT/$p.json"), $(jq -r '.execution_ms // "?"' "$OUT/$p.json") ms, $(jq -r '"\(.result.width)x\(.result.height)"' "$OUT/$p.json"), sent cap_$p's words, encoded after the restoration request"
done
{ echo "## caption on $NAME ($SKU)"; for p in "${PICTURES[@]}"; do echo "- $p: $(tr '\n' ' ' < "$OUT/cap_$p.txt" 2> /dev/null)"; done; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
exit "$RC"
