# orbit-arcs (se-152, proof branch only): renders on one rental, each a planned question, for general camera arcs. Paul, 11-17: "I want to
# give it a manga panel, and define where I want the motion to be able to arc around ... and follow an arc other than just a turn-table".
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), its first-party packages from the
# packages_refs checkout `tree` (orbit.sh's way), the rental held on the RC's builds (upscale.sh's way). Each render is one `cozy run`
# of a checkout function (orbit/probe, minimax-h3-image/probe); a failed one is said and the next goes on. Once the rental ends,
# fixtures/orbit-arcs/measure.py makes each video's stills, sheet and numbers on this runner. Without `rent`, $0: the pictures derived,
# the measurer checked, the quote. params (a JSON file in this checkout: `file`):
#   tree     the packages_refs checkout (required)
#   derive   pictures made here first: a list of {name, op: grey|hflip, from}, `from` a source (below)
#   renders  a list of {name, tool, files: {field: source}, fields: {field: value}, flip?, key?}: `flip` mirrors every picture in and
#            the video out (a clockwise LoRA turning the other way), `key` (white) keys measure.py's grey backdrop to it
#   seconds  what the cap must buy (default 7200)
# A source: fixture:<file> (fixtures/orbit-arcs/), derive:<name>, run:<name> (an earlier render's output) or frame:<name>@<n>
# (frame n of an earlier render's video).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/orbit-arcs WORK=${RUNNER_TEMP:-$OUT/..}/orbit-arcs
TREE=$(param tree '') RENDERS=$(param renders '[]') DERIVE=$(param derive '[]') NEED=$(param seconds 7200)
for tool in jq uv ffmpeg ffprobe; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/orbit/pyproject.toml" ] || fail "tree: a packages_refs checkout with orbit"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.tool | test("^[a-z0-9.-]+/[a-z_]+$")))' \
  <<<"$RENDERS" > /dev/null || fail "renders: a list of {name, tool, files?, fields?, flip?, key?}"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
say "packages from $(git -C "$SOURCE" rev-parse --short HEAD): $(grep -c '^async def probe' "$SOURCE/orbit/orbit/__init__.py") orbit probe"
for p in $(jq -r '.[].tool | split("/")[0]' <<<"$RENDERS" | sort -u) minimax-h3; do kernels "$SOURCE/$p"; done
mkdir -p "$WORK/derived" "$OUT/renders" "$OUT/records" "$OUT/inputs"

# The measurer's Python (numpy, Pillow), proven on a made-up clip before anything is rented.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 && uv pip install -q --python "$WORK/py/bin/python" numpy pillow >> "$OUT/py.log" 2>&1 ||
  fail "the measurer's Python: $(tail -c 300 "$OUT/py.log")"
PY=$WORK/py/bin/python
mkdir -p "$WORK/selftest"
ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=24:duration=2 -pix_fmt yuv420p "$WORK/selftest/t.mp4" &&
  "$PY" -I "$FIX/measure.py" video "$WORK/selftest/t.mp4" "$WORK/selftest/out" > "$OUT/selftest.txt" 2>&1 || fail "measure.py: $(tail -c 300 "$OUT/selftest.txt")"

# source <src>: the file a source names.
src() {
  case $1 in
    fixture:*) echo "$FIX/${1#fixture:}" ;;
    derive:*) echo "$WORK/derived/${1#derive:}.png" ;;
    run:*) ls "$OUT/renders/${1#run:}".* 2> /dev/null | grep -v asmade | head -1 ;;
    frame:*@*) local v=${1#frame:} f; f=$WORK/frames/${v/@/-}.png; mkdir -p "$WORK/frames"
      [ -f "$f" ] || ffmpeg -v error -y -i "$OUT/renders/${v%@*}.mp4" -vf "select=eq(n\,${v#*@})" -frames:v 1 "$f" 2> /dev/null
      echo "$f" ;;
    *) return 1 ;;
  esac
}
for i in $(seq 0 $(($(jq length <<<"$DERIVE") - 1))); do
  d=$(jq -c ".[$i]" <<<"$DERIVE"); name=$(jq -r .name <<<"$d") op=$(jq -r .op <<<"$d") from=$(src "$(jq -r .from <<<"$d")")
  [ -f "$from" ] || fail "derive $name: no $(jq -r .from <<<"$d")"
  "$PY" -I "$FIX/measure.py" "$op" "$from" "$WORK/derived/$name.png" > "$OUT/derive-$name.txt" 2>&1 || fail "derive $name: $(tail -c 300 "$OUT/derive-$name.txt")"
  cp "$WORK/derived/$name.png" "$OUT/inputs/"; say "derived $name: $op of $(jq -r .from <<<"$d")"
done
for f in "$FIX"/*.png "$FIX"/*.jpg; do [ -f "$f" ] && cp "$f" "$OUT/inputs/"; done

# A dev home, its first-party packages from the checkout (orbit.sh).
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

bad=0
for i in $(seq 0 $(($(jq length <<<"$RENDERS") - 1))); do
  r=$(jq -c ".[$i]" <<<"$RENDERS")
  name=$(jq -r .name <<<"$r") tool=$(jq -r .tool <<<"$r") flip=$(jq -r '.flip // false' <<<"$r")
  mapfile -t fields < <(jq -r '(.fields // {}) | to_entries[] | "\(.key):=\(.value | tojson)"' <<<"$r")
  files=() missing=''
  while read -r field source; do
    [ -n "$field" ] || continue
    f=$(src "$source") && [ -f "$f" ] || { missing="$missing $field=$source"; continue; }
    if [ "$flip" = true ]; then "$PY" -I "$FIX/measure.py" hflip "$f" "$WORK/$name-$field.png" > /dev/null 2>&1 && f=$WORK/$name-$field.png; fi
    files+=(--file "$field=$f")
  done < <(jq -r '(.files // {}) | to_entries[] | "\(.key) \(.value)"' <<<"$r")
  [ -z "$missing" ] || { say "SKIP $name: no$missing"; bad=1; continue; }
  say "render $name: $tool $(jq -c '{files, flip, key} | with_entries(select(.value != null))' <<<"$r") $(jq -c '(.fields // {}) | del(.text)' <<<"$r")"
  mkdir -p "$WORK/raw/$name"
  step "$name" run "$tool" "${files[@]}" "${fields[@]}" --on "$NAME" -y -o "$WORK/raw/$name"
  code=$? n=$(runno "$name")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$name.json" 2> /dev/null
  made=$(find "$WORK/raw/$name" -type f \( -name '*.mp4' -o -name '*.png' -o -name '*.webp' \) 2> /dev/null | head -1)
  if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $name: run ${n:-none}, exit $code: $(tail -c 400 "$OUT/$name.txt" | tr '\n' ' ')"; bad=1; continue; fi
  ext=${made##*.}
  if [ "$flip" = true ] && [ "$ext" = mp4 ]; then
    cp "$made" "$OUT/renders/$name-asmade.mp4"
    ffmpeg -v error -y -i "$made" -vf hflip -c:v libx264 -crf 14 -g 1 -pix_fmt yuv420p -an "$OUT/renders/$name.mp4" || { say "FAIL $name: hflip"; bad=1; continue; }
  else
    cp "$made" "$OUT/renders/$name.$ext"
  fi
  say "$name: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, total \(((.settled_at_ms // 0) - (.created_at_ms // 0)) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB, warnings \(.result.warnings // [] | length)"' "$OUT/records/$name.json" 2> /dev/null)"
  jq -r '.result.warnings // [] | .[] | "    warning: \(.)"' "$OUT/records/$name.json" 2> /dev/null | cut -c1-400 | tee -a "$R" > /dev/null
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 6h -n 0 > "$OUT/machine.log" 2>&1
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

# Each video's stills, sheet and numbers; a picture's sheet is itself.
mkdir -p "$OUT/stills" "$OUT/sheets"
for i in $(seq 0 $(($(jq length <<<"$RENDERS") - 1))); do
  r=$(jq -c ".[$i]" <<<"$RENDERS"); name=$(jq -r .name <<<"$r") key=$(jq -r '.key // empty' <<<"$r")
  v=$OUT/renders/$name.mp4
  [ -f "$v" ] || continue
  "$PY" -I "$FIX/measure.py" video "$v" "$OUT/stills/$name" ${key:+--key "$key"} > "$OUT/measure-$name.txt" 2>&1 || { say "measure $name: $(tail -c 300 "$OUT/measure-$name.txt")"; bad=1; continue; }
  cp "$OUT/stills/$name/sheet.jpg" "$OUT/sheets/$name.jpg"
  [ -z "$key" ] || cp "$OUT/stills/$name/sheet-keyed.jpg" "$OUT/sheets/$name-keyed.jpg"
  say "measure $name: $(tail -1 "$OUT/measure-$name.txt")"
done
{ echo "## orbit-arcs on $NAME ($SKU)"; echo '```'; grep -E '^[0-9:]+ (render|measure|FAIL|SKIP|[a-z0-9-]+: run)' "$R"; echo '```'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$bad" = 0 ] || RC=1
exit "$RC"
