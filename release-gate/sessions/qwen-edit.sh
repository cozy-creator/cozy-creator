# qwen-edit (se-153, proof branch only): Qwen Image 2.1 edits of public photos on one rental: which settings soften an edit, and
# what a chain of edits loses. Paul, 11-17: "the 2-megapixel image outputted looks a little low-res … not that sharp and has jpeg
# artifacts or something"; "even at 4MP it's not doing a good job of recovering the detail"; "if you use images as references, like
# A -> B -> C and keep editing them, it introduces noise".
# The bench (fixtures/qwen-edit, sources.tsv): Wikimedia Commons photos, sha256-checked; each edit's input is a 1080x1350 JPEG q85,
# each chain starts from a 1280x1600 PNG. A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of
# rc/<x>), the rental held on the RC's builds (upscale.sh's way); each run is `cozy run <packages_refs checkout>/qwen-image-2/<fn>`.
# Once the rental ends, fixtures/qwen-edit/measure.py scores every output on this runner: the kept regions against the clean
# photo (DISTS, LPIPS, after alignment), their high-frequency energy, colour shift, zoom and shift, and each chain's generations
# against its generation 0; sheets of crops at 100% and 200% for the eye. Without `rent`, $0: the bench built, the scorer proven
# on it, the quote.
# params (a JSON file in this checkout: `file`): cases {group: [{name, input, prompt, seed, kind: keep|gray|none, boxes?,
# aspect_ratio?}]}, arms [{name, tree, fn, fields?, flags?, sets: [{cases: group, mps}]}], chains [{name, start, steps: [{prompt,
# seed}]}], chain_arms [{name, tree, fn, fields?, flags?}], chain_mp, seconds (what the cap must buy). A tree is a packages_refs
# checkout's name.
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/qwen-edit WORK=${RUNNER_TEMP:-$OUT/..}/qwen-edit
NEED=$(param seconds 10800) CHAIN_MP=$(param chain_mp 2)
for tool in jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
jq -e '(.arms | type == "array" and length > 0) and (.cases | type == "object")' <<<"$PARAMS" > /dev/null || fail "params: arms and cases"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
for tree in $(jq -r '[.arms[].tree, (.chain_arms // [])[].tree] | unique[]' <<<"$PARAMS"); do
  [ -f "$PACKAGES/$tree/qwen-image-2/pyproject.toml" ] || fail "no qwen-image-2 in packages_refs checkout $tree"
  say "tree $tree: $(git -C "$PACKAGES/$tree" log --oneline -1)"
  kernels "$PACKAGES/$tree/qwen-image-2"
done
mkdir -p "$WORK/raw" "$OUT/records" "$OUT/runs" "$OUT/chains"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
# `source_tree`: a packages_refs checkout whose packages a package from source takes its in-repo dependencies from
# (Calcifer's `packages`, orbit-arcs.sh's way): qwen-image-2 takes birefnet before birefnet's release is on the Hub.
ST=$(param source_tree '')
if [ -n "$ST" ]; then
  SOURCE=$(cd "$PACKAGES/$ST" && pwd) || fail "source_tree $ST: no packages_refs checkout"
  awk -v source="$SOURCE" '
    function ours() { print "packages = \"" source "\""; put = 1 }
    /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
fi
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
# Each function as a machine will read it, before anything is rented (a package surface the static reader refuses
# failed every run of session 44188228407's proof tree).
while read -r tree fn; do
  (cd "$RUN_PROJECT" && cozy run "$PACKAGES/$tree/qwen-image-2/$fn" --describe) > "$OUT/describe-$tree-$fn.txt" 2>&1 ||
    fail "$tree/qwen-image-2/$fn: $(head -c 300 "$OUT/describe-$tree-$fn.txt")"
done < <(jq -r '[.arms[], (.chain_arms // [])[] | "\(.tree) \(.fn)"] | unique[]' <<<"$PARAMS")
say "described: $(jq -r '[.arms[], (.chain_arms // [])[] | "\(.tree)/\(.fn)"] | unique | join(" ")' <<<"$PARAMS")"

# The scorer's Python (CPU torch, pyiqa, OpenCV) and the bench, proven on the inputs before anything is rented.
uv venv -q --python 3.12 "$WORK/iqa" > "$OUT/iqa.log" 2>&1 &&
  uv pip install -q --python "$WORK/iqa/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' opencv-python-headless >> "$OUT/iqa.log" 2>&1 ||
  fail "the scorer's Python: $(tail -c 400 "$OUT/iqa.log")"
PY=$WORK/iqa/bin/python
"$PY" -I "$FIX/make_set.py" --fetch > "$OUT/make-set.log" 2>&1 || fail "the bench: $(tail -c 400 "$OUT/make-set.log")"
say "bench: $(ls "$FIX/inputs" | paste -sd' ')"
"$PY" -I "$FIX/measure.py" --plan <(jq -c . <<<"$PARAMS") --work "$WORK" --out "$OUT/selftest" --selftest > "$OUT/selftest.txt" 2>&1 ||
  fail "measure.py: $(tail -c 400 "$OUT/selftest.txt")"

[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT

quote "$NEED"
order
ready
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
for try in 1 2 3; do
  step "hold-$try" machine update "$NAME" --runtime "$(jq -r '.status.runtime' "$OUT/show-rc.json")" -y
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && { [ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ]; } && break
    sleep 5
  done
  [ -z "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || break
done
[ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || fail "$NAME isn't held on the RC's builds: $(tail -c 300 "$OUT/hold-$try.txt")"
say "held: $(jq -r '.held' "$OUT/show-rc.json")"

# edit <run> <arm JSON> <picture> <prompt> <seed> <megapixels> <aspect ratio or ''> <into>: one run; its picture lands at <into>.<ext>.
bad=0
edit() {
  local s=$1 arm=$2 picture=$3 prompt=$4 seed=$5 mp=$6 ratio=$7 into=$8 tree fn fields flags made code n
  tree=$(jq -r .tree <<<"$arm") fn=$(jq -r .fn <<<"$arm")
  mapfile -t fields < <(jq -r '(.fields // {}) | to_entries[] | "\(.key):=\(.value | tojson)"' <<<"$arm")
  mapfile -t flags < <(jq -r '(.flags // [])[]' <<<"$arm")
  [ -z "$ratio" ] || fields+=("aspect_ratio=$ratio")
  mkdir -p "$WORK/raw/$s"
  step "$s" run "$PACKAGES/$tree/qwen-image-2/$fn" "prompt=$prompt" \
    "references:=$(jq -cn --arg i "$picture" '[{name: "photo", image: $i}]')" "seed:=$seed" "megapixels:=$mp" prompt_upsampling:=false \
    "${fields[@]}" "${flags[@]}" --on "$NAME" -y -o "$WORK/raw/$s"
  code=$? n=$(runno "$s")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$s.json" 2> /dev/null
  made=$(find "$WORK/raw/$s" -type f \( -name '*.webp' -o -name '*.png' -o -name '*.jpg' \) 2> /dev/null | head -1)
  if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $s: run ${n:-none}, exit $code: $(tail -c 300 "$OUT/$s.txt" | tr '\n' ' ')"; bad=1; return 1; fi
  mkdir -p "$(dirname "$into")" && cp "$made" "$into.${made##*.}"
  say "$s: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, generating \((.measurements.attribution.stages["generating image"].total_ms // 0) / 1000 * 100 | floor / 100) s, lanes \([.measurements.models[]?.lane] | unique | join("+")), attention \(.measurements.execution.gpus[0].attention.observed // "?")"' "$OUT/records/$s.json" 2> /dev/null)"
}

run_arms() {
for i in $(seq 0 $(($(jq '.arms | length' <<<"$PARAMS") - 1))); do
  arm=$(jq -c ".arms[$i]" <<<"$PARAMS"); a=$(jq -r .name <<<"$arm")
  say "arm $a: $(jq -c 'del(.sets)' <<<"$arm")"
  while read -r group mps; do
    for j in $(seq 0 $(($(jq --arg g "$group" '.cases[$g] | length' <<<"$PARAMS") - 1))); do
      c=$(jq -c --arg g "$group" ".cases[\$g][$j]" <<<"$PARAMS")
      name=$(jq -r .name <<<"$c") input=$(jq -r .input <<<"$c")
      for mp in ${mps//,/ }; do
        edit "$a--$name--$mp" "$arm" "$FIX/inputs/$input.jpg" "$(jq -r .prompt <<<"$c")" "$(jq -r .seed <<<"$c")" "$mp" \
          "$(jq -r '.aspect_ratio // empty' <<<"$c")" "$OUT/runs/$a/$name--$mp"
      done
    done
  done < <(jq -r '.sets[] | "\(.cases) \(.mps | join(","))"' <<<"$arm")
done
}
# Each chain: generation k edits generation k-1's own file, as a box referencing a take does.
run_chains() {
for i in $(seq 0 $(($(jq '(.chains // []) | length' <<<"$PARAMS") - 1))); do
  chain=$(jq -c ".chains[$i]" <<<"$PARAMS"); cn=$(jq -r .name <<<"$chain")
  for k in $(seq 0 $(($(jq '(.chain_arms // []) | length' <<<"$PARAMS") - 1))); do
    arm=$(jq -c ".chain_arms[$k]" <<<"$PARAMS"); a=$(jq -r .name <<<"$arm")
    previous=$FIX/chain/$(jq -r .start <<<"$chain")-0.png
    for g in $(seq 1 "$(jq '.steps | length' <<<"$chain")"); do
      st=$(jq -c ".steps[$((g - 1))]" <<<"$chain")
      edit "chain-$cn--$a--g$g" "$arm" "$previous" "$(jq -r .prompt <<<"$st")" "$(jq -r .seed <<<"$st")" "$CHAIN_MP" "" \
        "$OUT/chains/$cn/$a/g$g" || break
      previous=$(ls "$OUT/chains/$cn/$a/g$g".* | head -1)
    done
  done
done
}
# `chains_first`: the chains before the single edits, so a deadline cuts the single edits.
if [ "$(param chains_first false)" = true ]; then run_chains; run_arms; else run_arms; run_chains; fi
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 6h -n 0 > "$OUT/machine.log" 2>&1
# The rental ends before the scoring, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

"$PY" -I "$FIX/measure.py" --plan <(jq -c . <<<"$PARAMS") --work "$OUT" --records "$OUT/records" --out "$OUT/scores" > "$OUT/measure.txt" 2>&1 ||
  { say "measure.py: $(tail -c 400 "$OUT/measure.txt")"; RC=1; }
{ echo "## qwen-edit on $NAME ($SKU)"; cat "$OUT/scores/summary.md" 2> /dev/null; } > "$OUT/summary.md"
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
sed 's/^/  /' "$OUT/summary.md" | tee -a "$R" > /dev/null
[ "$bad" = 0 ] || RC=1
exit "$RC"
