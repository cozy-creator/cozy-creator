# qwen-cutout (se-154): Qwen Image 2's "remove the background" on portraits and a product photo, from packages_refs
# checkouts on one rental, each output's alpha judged on this runner (fixtures/qwen-cutout/judge.py) and laid out in sheets.
# se-154, Paul 11-17, on a Qwen Image 2.1 edit asked to "remove the background": "for v8 it really got corrupted or
# something with noise I don't know what happened there look into it".
# Inputs: fixtures/ltx-images' pearl.jpg (a portrait on a dark ground, public domain) and teapot.jpg (a product photo), and
# pictures a cell makes first (a cell with no `input`; synthetic portraits on a studio wall, as Paul's v7 was).
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), as upscale: the rental moves
# to the RC's builds and is held there. Without `rent`, $0: the judge proven on the fixtures, the quote, nothing rented.
# A tree whose qwen-image-2 takes its checkout's birefnet from the Hub runs from a copy taking it by path (lib.sh `step`).
# params:
#   cells    a JSON list, run in order, of {name, tree, input?, fields}: `tree` a packages_refs checkout, `input` a fixture
#            (pearl, teapot; stray, Pearl as RGBA with 40 pixels at alpha 215-250, as an opaque Qwen edit leaves them) or
#            `@<cell>` (that cell's picture), given as <p1>; `fields` the image tool's inputs (`key:=value` each;
#            `instructions` names <p1>). Every cell runs `qwen-image-2/image`.
#   install  `true`: before the quote, each tree's qwen-image-2 is installed on this runner's own machine (`local`, $0),
#            through `step` as a run is: its lock and its first-party dependencies, with no rental
#   judged   the cells judge.py holds to a clean cutout (comma-separated; default every cell with an input): judge.py's bars,
#            a FAIL fails the session
#   seconds  what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/qwen-cutout WORK=${RUNNER_TEMP:-$OUT/..}/qwen-cutout
CELLS=$(param cells '[]') JUDGED=$(param judged '') NEED=$(param seconds 3600) INSTALL=$(param install false)
for tool in jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.tree | type == "string") and
  ((.input // "") | test("^(|pearl|teapot|stray|@[a-z0-9-]+)$")) and (.fields | type == "object"))' <<<"$CELLS" > /dev/null ||
  fail "cells: a JSON list of {name, tree, input?, fields}"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
mkdir -p "$WORK/raw" "$OUT/records" "$OUT/runs" "$OUT/sheets"
# Each tree's qwen-image-2, its Triton sets built before anything is rented.
for tree in $(jq -r '.[].tree' <<<"$CELLS" | sort -u); do
  [ -f "$PACKAGES/$tree/qwen-image-2/pyproject.toml" ] || fail "no qwen-image-2 in packages_refs checkout $tree"
  kernels "$PACKAGES/$tree/qwen-image-2"
  say "tree $tree: $(git -C "$PACKAGES/$tree" rev-parse --short HEAD), qwen-image-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$PACKAGES/$tree/qwen-image-2/pyproject.toml"), Hub sources: $(grep -E '= *\{ *index *= *"tensorhub"' "$PACKAGES/$tree/qwen-image-2/pyproject.toml" | cut -d' ' -f1 | paste -sd' ')"
done
# The judge's Python, proven on the fixtures before anything is rented.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 && uv pip install -q --python "$WORK/py/bin/python" numpy pillow >> "$OUT/py.log" 2>&1 ||
  fail "the judge's Python: $(tail -c 400 "$OUT/py.log")"
PY=$WORK/py/bin/python
"$PY" -I "$FIX/judge.py" --self-check "$FIX/../ltx-images/teapot.jpg" > "$OUT/judge-check.txt" 2>&1 || fail "judge.py: $(tail -c 400 "$OUT/judge-check.txt")"
"$PY" -I -c 'import sys; import numpy as np; from PIL import Image
a = np.dstack([np.asarray(Image.open(sys.argv[1]).convert("RGB")), np.full(Image.open(sys.argv[1]).size[::-1], 255, np.uint8)])
r = np.random.default_rng(0); a[r.integers(0, a.shape[0], 40), r.integers(0, a.shape[1], 40), 3] = r.integers(215, 251, 40)
Image.fromarray(a).save(sys.argv[2])' "$HERE/fixtures/ltx-images/pearl.jpg" "$WORK/stray.png" || fail "the stray-alpha picture"

# A dev home: the rental moves to the RC's builds.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
run_project "$RUN_PROJECT" || fail "project at $RUN_PROJECT"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
if [ "$INSTALL" = true ]; then
  for tree in $(jq -r '.[].tree' <<<"$CELLS" | sort -u); do
    step "install-$tree" package install "$PACKAGES/$tree/qwen-image-2" --on local -y ||
      fail "tree $tree's qwen-image-2 did not install here: $(grep -v '^ *$' "$OUT/install-$tree.txt" | tail -5 | tr '\n' ' ' | cut -c1-600)"
    say "tree $tree's qwen-image-2 installed here: $(tail -2 "$OUT/install-$tree.txt" | tr '\n' ' ' | cut -c1-300)"
  done
fi
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

# Each cell in order; a failed run is said and the next goes on.
bad=0
: > "$WORK/cells.tsv"
for i in $(seq 0 $(($(jq length <<<"$CELLS") - 1))); do
  c=$(jq -c ".[$i]" <<<"$CELLS")
  name=$(jq -r .name <<<"$c") tree=$(jq -r .tree <<<"$c") input=$(jq -r '.input // empty' <<<"$c")
  mapfile -t fields < <(jq -r '.fields | to_entries[] | "\(.key):=\(.value | tojson)"' <<<"$c")
  src=''
  case $input in
    '') ;;
    @*) src=$(find "$WORK/raw/${input#@}" -type f \( -name '*.webp' -o -name '*.png' \) 2> /dev/null | head -1) ;;
    stray) src=$WORK/stray.png ;;
    *) src=$HERE/fixtures/ltx-images/$input.jpg ;;
  esac
  [ -z "$input" ] || [ -f "$src" ] || { say "SKIP $name: no input $input"; bad=1; continue; }
  refs=(); [ -z "$src" ] || refs=("references:=[{\"name\":\"p1\",\"image\":\"$src\"}]")
  mkdir -p "$WORK/raw/$name"
  step "$name" run "$PACKAGES/$tree/qwen-image-2/image" "${fields[@]}" "${refs[@]}" --on "$NAME" -y -o "$WORK/raw/$name"
  code=$? n=$(runno "$name")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$name.json" 2> /dev/null
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/records/$name.txt" 2> /dev/null  # its log
  made=$(find "$WORK/raw/$name" -type f \( -name '*.webp' -o -name '*.png' \) 2> /dev/null | head -1)
  if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $name: run ${n:-none}, exit $code: $(tail -c 300 "$OUT/$name.txt" | tr '\n' ' ')"; bad=1; continue; fi
  cp "$made" "$OUT/runs/$name.${made##*.}"
  printf '%s\t%s\t%s\t%s\n' "$name" "$tree" "${src:-}" "$OUT/runs/$name.${made##*.}" >> "$WORK/cells.tsv"
  say "$name: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, total \(((.settled_at_ms // 0) - (.created_at_ms // 0)) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB"' "$OUT/records/$name.json" 2> /dev/null)"
  jq -r '[.result.prompt // empty, (.warnings // [] | map("warning: " + (.message // tostring)) | .[])] | .[] | "    " + .[0:600]' "$OUT/records/$name.json" 2> /dev/null | tee -a "$R"
  grep -E '^ *info: cutout ' "$OUT/records/$name.txt" 2> /dev/null | sed 's/^ */    /' | tee -a "$R"
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 4h -n 0 > "$OUT/machine.log" 2>&1
# The rental ends before the judging, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

judged=$JUDGED
[ -n "$judged" ] || judged=$(jq -r '[.[] | select((.input // "") != "") | .name] | join(",")' <<<"$CELLS")
"$PY" -I "$FIX/judge.py" "$WORK/cells.tsv" --judged "$judged" --sheets "$OUT/sheets" --out "$OUT/judge.tsv" > "$OUT/judge.txt" 2>&1 || RC=1
sed 's/^/  /' "$OUT/judge.txt" | tee -a "$R"
{ echo "## qwen-cutout on $NAME ($SKU)"; echo '```'; cat "$OUT/judge.txt"; echo '```'; } > "$OUT/summary.md"
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$bad" = 0 ] || RC=1
exit "$RC"
