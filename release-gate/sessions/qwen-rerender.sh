# qwen-rerender (se-153, proof branch only): what re-rendering a picture does to it, by cause, on one rental. The last
# agent measured that a region an edit regenerates gets sharper each time a later edit re-renders it (a shirt edited at
# step one carries 3.4x its first render's Laplacian by the fifth edit under Turbo, 2.2x under the base model), and that
# an edit that changes the framing can't use paste-back (`keep_unchanged`).
# Each sequence starts from a bench picture (fixtures/qwen-edit, sources.tsv: Wikimedia Commons photos) or from another
# sequence's picture, and each step runs one function on the picture before it: an edit of it (`prompt`, the picture as
# <photo>), or the VAE alone (no `prompt`: `--file image=`, the proof function `vae_roundtrip`). Sequences that share a
# start differ only in what they name (function, fields, flags), so the causes separate: the VAE round trip with no
# denoising, Turbo against the base model on the same picture, the base model at more steps.
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), as upscale: the rental
# moves to the RC's builds and is held there. Once the rental ends, fixtures/qwen-edit/rerender.py scores every picture
# on this runner. Without `rent`, $0: the bench built, every function described, the scorer proven, the quote.
# params (a JSON file in this checkout: `file`):
#   sequences  a list, run in order, of {name, tree, fn, start, fields?, flags?, boxes?, steps: [{prompt?, seed?,
#              aspect_ratio?, fields?}]}: `tree` a packages_refs checkout; `start` a bench file (`chain/miyara-0.png`) or
#              `@<sequence>/g<k>`; `fields` the function's inputs on every step, a step's own over them; `flags` more
#              `cozy run` flags; `boxes` {name: [x0, y0, x1, y1]} as fractions, what rerender.py measures
#   seconds    what the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/qwen-edit WORK=${RUNNER_TEMP:-$OUT/..}/qwen-rerender
SEQS=$(param sequences '[]') NEED=$(param seconds 1800)
for tool in jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.tree | type == "string") and
  (.fn | test("^[a-z0-9_]+$")) and (.start | test("^(@[a-z0-9-]+/g[0-9]+|[a-z]+/[a-z0-9-]+\\.(png|jpg))$")) and
  (.steps | type == "array" and length > 0))' <<<"$SEQS" > /dev/null || fail "sequences: a list of {name, tree, fn, start, steps}"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
mkdir -p "$WORK/raw" "$OUT/records" "$OUT/seq"
for tree in $(jq -r '.[].tree' <<<"$SEQS" | sort -u); do
  [ -f "$PACKAGES/$tree/qwen-image-2/pyproject.toml" ] || fail "no qwen-image-2 in packages_refs checkout $tree"
  kernels "$PACKAGES/$tree/qwen-image-2"
  say "tree $tree: $(git -C "$PACKAGES/$tree" log --oneline -1 | cut -c1-120)"
done
# The bench and the scorer's Python (NumPy, Pillow), proven before anything is rented.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 && uv pip install -q --python "$WORK/py/bin/python" numpy pillow >> "$OUT/py.log" 2>&1 ||
  fail "the scorer's Python: $(tail -c 400 "$OUT/py.log")"
PY=$WORK/py/bin/python
"$PY" -I "$FIX/make_set.py" --fetch > "$OUT/make-set.log" 2>&1 || fail "the bench: $(tail -c 400 "$OUT/make-set.log")"
for start in $(jq -r '.[].start | select(startswith("@") | not)' <<<"$SEQS" | sort -u); do
  [ -f "$FIX/$start" ] || fail "no bench picture $start"
done
"$PY" -I "$FIX/rerender.py" --self-check "$FIX/chain/miyara-0.png" > "$OUT/self-check.txt" 2>&1 || fail "rerender.py: $(tail -c 400 "$OUT/self-check.txt")"

# A dev home: the rental moves to the RC's builds.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
run_project "$RUN_PROJECT" || fail "project at $RUN_PROJECT"
# Each function as a machine will read it, before anything is rented.
while read -r tree fn; do
  (cd "$RUN_PROJECT" && cozy run "$PACKAGES/$tree/qwen-image-2/$fn" --describe) > "$OUT/describe-$tree-$fn.txt" 2>&1 ||
    fail "$tree/qwen-image-2/$fn: $(head -c 300 "$OUT/describe-$tree-$fn.txt")"
done < <(jq -r '[.[] | "\(.tree) \(.fn)"] | unique[]' <<<"$SEQS")
say "described: $(jq -r '[.[] | "\(.tree)/\(.fn)"] | unique | join(" ")' <<<"$SEQS")"
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
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
# Held there (upscale): the RC follower would move it to each newer RC build between runs, each move a cold start.
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

# Each sequence in order, each step on the picture before it; a failed step ends its sequence and the next goes on.
bad=0
for i in $(seq 0 $(($(jq length <<<"$SEQS") - 1))); do
  q=$(jq -c ".[$i]" <<<"$SEQS")
  name=$(jq -r .name <<<"$q") tree=$(jq -r .tree <<<"$q") fn=$(jq -r .fn <<<"$q") start=$(jq -r .start <<<"$q")
  mapfile -t flags < <(jq -r '(.flags // [])[]' <<<"$q")
  case $start in
    @*) picture=$(ls "$OUT/seq/${start#@}".* 2> /dev/null | head -1) ;;
    *) picture=$FIX/$start ;;
  esac
  [ -f "$picture" ] || { say "SKIP $name: no start $start"; bad=1; continue; }
  mkdir -p "$OUT/seq/$name"
  cp "$picture" "$OUT/seq/$name/g0.${picture##*.}"
  say "sequence $name: $tree/$fn from $start $(jq -c '{fields: (.fields // {}), flags: (.flags // [])}' <<<"$q")"
  for g in $(seq 1 "$(jq '.steps | length' <<<"$q")"); do
    st=$(jq -c ".steps[$((g - 1))]" <<<"$q") s=$name--g$g
    mapfile -t fields < <(jq -rn --argjson q "$q" --argjson st "$st" '(($q.fields // {}) + ($st.fields // {})) | to_entries[] | "\(.key):=\(.value | tojson)"')
    prompt=$(jq -r '.prompt // empty' <<<"$st")
    if [ -n "$prompt" ]; then
      args=("prompt=$prompt" "references:=$(jq -cn --arg i "$picture" '[{name: "photo", image: $i}]')" prompt_upsampling:=false)
      seed=$(jq -r '.seed // empty' <<<"$st"); [ -z "$seed" ] || args+=("seed:=$seed")
      ratio=$(jq -r '.aspect_ratio // empty' <<<"$st"); [ -z "$ratio" ] || args+=("aspect_ratio=$ratio")
    else
      args=(--file "image=$picture")
    fi
    mkdir -p "$WORK/raw/$s"
    step "$s" run "$PACKAGES/$tree/qwen-image-2/$fn" "${args[@]}" "${fields[@]}" "${flags[@]}" --on "$NAME" -y -o "$WORK/raw/$s"
    code=$? n=$(runno "$s")
    [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$s.json" 2> /dev/null
    [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/records/$s.txt" 2> /dev/null  # its log
    made=$(find "$WORK/raw/$s" -type f \( -name '*.webp' -o -name '*.png' \) 2> /dev/null | head -1)
    if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $s: run ${n:-none}, exit $code: $(tail -c 300 "$OUT/$s.txt" | tr '\n' ' ')"; bad=1; break; fi
    picture=$OUT/seq/$name/g$g.${made##*.}
    cp "$made" "$picture"
    say "$s: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, generating \((.measurements.attribution.stages["generating image"].total_ms // 0) / 1000 * 100 | floor / 100) s, \(.result.width // "?")x\(.result.height // "?"), lanes \([.measurements.models[]?.lane] | unique | join("+")), attention \(.measurements.execution.gpus[0].attention.observed // "?"), peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB"' "$OUT/records/$s.json" 2> /dev/null)"
    grep -E '^ *info: (keep unchanged|cutout|sampling) ' "$OUT/records/$s.txt" 2> /dev/null | sed 's/^ */    /' | tee -a "$R"
  done
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
# The rental ends before the scoring, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

"$PY" -I "$FIX/rerender.py" --plan <(jq -c . <<<"$SEQS") --seq "$OUT/seq" --out "$OUT/scores.tsv" > "$OUT/scores.txt" 2>&1 ||
  { say "rerender.py: $(tail -c 400 "$OUT/scores.txt")"; RC=1; }
{ echo "## qwen-rerender on $NAME ($SKU)"; echo '```'; cat "$OUT/scores.txt"; echo '```'; } > "$OUT/summary.md"
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
sed 's/^/  /' "$OUT/scores.txt" | tee -a "$R" > /dev/null
[ "$bad" = 0 ] || RC=1
exit "$RC"
