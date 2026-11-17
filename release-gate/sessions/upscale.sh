# upscale (se-147, se-149): picture Upscale tools on small JPEGs, scored against the clean pictures they were made from, on one rental.
# se-147, Paul 11-17: "upscaling doesn't work. I attached the qwen image 2.1 upscaler to an image and ran it, and it just made the image
# more of the same. The image was small and had JPEG artifacts; those JPG artifacts just became more embellished, rather than fixed (the
# image was not upscaled at all properly)."
# se-149, Paul 11-17, on the other picture upscalers: "audit the other upscalers to ensure they're actually doing useful upscaling
# ... ensure we're using them as recommended, to get maximal results. Ideally for an upscaler we should have the option to make it
# strict (keep to the results) or creative (enhance the image with details as needed)."
# The bench (fixtures/upscale-bench, its README): degraded/<name>.jpg, 225 px JPEGs (q50-75) of clean originals that make_set.py rebuilds
# here from sources.tsv: the originals from the private benchmarks checkout's upscale/originals (`bench_ref` its branch), else
# Wikimedia Commons, each checked by sha256. Each candidate upscales each input on the rental; metrics.py scores
# every output against its clean picture (PSNR, LPIPS, DISTS) and with none (MUSIQ, CLIP-IQA), and sheets.py lays them side by side
# ($OUT/sheets: each at 640 px, and a 640 px window at full size). Scores in $OUT/scores.tsv, means in summary.md and the job summary.
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), as llm-tools: the rental moves to the
# RC's builds and is held there. Without `rent`, $0: the bench is built and the Lanczos floor scored, the quote, nothing rented.
# params:
#   candidates  a JSON list, run in order, of {name, tree, tool, fields?, from?, images?}: `tree` a packages_refs checkout, `tool` its
#               <package>/<function>, `fields` its inputs (`key:=value` each), `from` a candidate whose outputs are this one's inputs,
#               `images` the inputs it takes (comma-separated names; default all)
#   images      the inputs (comma-separated names of degraded/; default all)
#   private     more inputs, kept private: a folder of <name>.jpg files in the private benchmarks checkout (`bench_ref` its branch;
#               never in params, whose values the public log prints). Their outputs and sheets leave the runner only encrypted to
#               `pubkey` (base64 of a PEM RSA public key): private.tar.gz.enc, its AES key in private.key.enc
#   bars        the cell's verdict: a JSON list of {candidate, vs, metric, by}: `candidate`'s mean DISTS or LPIPS over the
#               full-reference pictures it shares with `vs` (a candidate, or `lanczos`, the floor) at most `vs`'s times 1 - `by`
#               (bars.py, after the scoring); a bar missed fails the session. Set `by` from a measured gap, never to pass.
#   post        candidates made on this runner once the rental ends, scored with the rest: a JSON list of {candidate, methods}:
#               colorfix.py's colour matches (adain, wavelet, wavelet64) of `candidate`'s outputs to their inputs, each
#               `<candidate>+<method>`
#   seconds     what the cap must buy (default 5400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/upscale-bench WORK=${RUNNER_TEMP:-$OUT/..}/upscale
CANDIDATES=$(param candidates '[]') IMAGES=$(param images '') PRIVATE=$(param private '') PUBKEY=$(param pubkey '') NEED=$(param seconds 5400)
BARS=$(param bars '[]') POST=$(param post '[]')
for tool in jq uv openssl; do command -v "$tool" > /dev/null || fail "no $tool"; done
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.tree | type == "string") and (.tool | test("^[a-z0-9._-]+/[a-z_]+$")))' \
  <<<"$CANDIDATES" > /dev/null || fail "candidates: a JSON list of {name, tree, tool, fields?, from?, images?}"
[ -z "$PRIVATE" ] || [ -n "$PUBKEY" ] || fail "private inputs: name a pubkey to encrypt their outputs to"
jq -e --argjson c "$CANDIDATES" '([$c[].name] + ["lanczos"]) as $n | type == "array" and all(.[]; (.candidate as $x | $n | index($x)) and
  (.vs as $x | $n | index($x)) and (.metric | IN("dists", "lpips")) and (.by | type == "number" and . >= 0 and . < 1))' <<<"$BARS" > /dev/null ||
  fail "bars: a JSON list of {candidate, vs, metric: dists|lpips, by: 0-1}, each naming a candidate (vs: or lanczos)"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
while read -r tree tool; do
  [ -f "$PACKAGES/$tree/${tool%/*}/pyproject.toml" ] || fail "no ${tool%/*} in packages_refs checkout $tree"
  kernels "$PACKAGES/$tree/${tool%/*}"
done < <(jq -r '.[] | "\(.tree) \(.tool)"' <<<"$CANDIDATES" | sort -u)
mkdir -p "$WORK/runs" "$WORK/raw" "$OUT/records" "$OUT/runs" "$OUT/sheets"

# The bench, and the scorer's own Python (CPU torch).
uv venv -q --python 3.12 "$WORK/iqa" > "$OUT/iqa.log" 2>&1 &&
  uv pip install -q --python "$WORK/iqa/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' >> "$OUT/iqa.log" 2>&1 ||
  fail "the scorer's Python: $(tail -c 400 "$OUT/iqa.log")"
PY=$WORK/iqa/bin/python
[ -d "$BENCH/upscale/originals" ] || say "no upscale/originals in the benchmarks checkout (params.bench_ref): from Wikimedia"
"$PY" -I "$FIX/make_set.py" --from "$BENCH/upscale/originals" --fetch --clean-only > "$OUT/make-set.log" 2>&1 ||
  fail "the bench: $(tail -c 400 "$OUT/make-set.log")"
say "bench: $(ls "$FIX/clean" | wc -l) clean pictures, $(ls "$FIX/degraded" | wc -l) inputs"
mkdir -p "$FIX/paul"
if [ -n "$PRIVATE" ]; then
  [[ $PRIVATE =~ ^[A-Za-z0-9/_-]+$ ]] && ls "$BENCH/$PRIVATE"/*.jpg > /dev/null 2>&1 || fail "private: no <name>.jpg in the benchmarks checkout's $PRIVATE"
  cp "$BENCH/$PRIVATE"/*.jpg "$FIX/paul/" || fail "private: copy"
  base64 -d <<<"$PUBKEY" > "$WORK/pub.pem" && openssl pkey -pubin -in "$WORK/pub.pem" -noout || fail "pubkey: not a PEM public key"
  say "private: $(ls "$FIX/paul" | paste -sd' ')"
fi
# input <name>: the picture a first candidate takes.
input() { local f; for f in "$FIX/degraded/$1.jpg" "$FIX/paul/$1.jpg"; do [ -f "$f" ] && { echo "$f"; return 0; }; done; return 1; }
private() { [ -f "$FIX/paul/$1.jpg" ]; }
ALL=$( (cd "$FIX/degraded" && ls *.jpg; cd "$FIX/paul" && ls *.jpg 2> /dev/null) | sed 's/\.jpg$//' | paste -sd,)
IMAGES=${IMAGES:-$ALL}
# The floor, and the scorer proven before anything is rented.
"$PY" -I "$FIX/metrics.py" "$WORK/runs" --out "$OUT/scores-lanczos.tsv" > "$OUT/scores-lanczos.txt" 2>&1 ||
  fail "metrics.py: $(tail -c 400 "$OUT/scores-lanczos.txt")"
sed -n '/^candidate /,$p' "$OUT/scores-lanczos.txt" | sed 's/^/  /' | tee -a "$R"

# A dev home: the rental moves to the RC's builds.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
# The runs' project is outside $OUT, which is published: it holds every picture, the private ones too.
run_project "$WORK/project" || fail "project at $WORK/project"
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
# Held there (llm-tools): the RC follower would move it to each newer RC build between runs, each move a cold start.
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

# Each candidate on each of its inputs, in order; a failed run is said and the next goes on.
bad=0
for i in $(seq 0 $(($(jq length <<<"$CANDIDATES") - 1))); do
  c=$(jq -c ".[$i]" <<<"$CANDIDATES")
  name=$(jq -r .name <<<"$c") tree=$(jq -r .tree <<<"$c") tool=$(jq -r .tool <<<"$c") from=$(jq -r '.from // empty' <<<"$c")
  mapfile -t fields < <(jq -r '(.fields // {}) | to_entries[] | "\(.key):=\(.value | tojson)"' <<<"$c")
  mine=$(jq -r --arg d "$IMAGES" '.images // $d' <<<"$c")
  say "candidate $name: $tree/$tool ${fields[*]}${from:+, from $from}"
  mkdir -p "$WORK/runs/$name"
  for image in ${mine//,/ }; do
    [[ ",$IMAGES," == *",$image,"* ]] || continue
    if [ -n "$from" ]; then src=$WORK/runs/$from/$image.png; else src=$(input "$image"); fi
    [ -f "$src" ] || { say "SKIP $name $image: no input ${from:+from $from}"; continue; }
    s=$name--$image
    mkdir -p "$WORK/raw/$s"
    step "$s" run "$PACKAGES/$tree/$tool" --file "image=$src" "${fields[@]}" --on "$NAME" -y -o "$WORK/raw/$s"
    code=$? n=$(runno "$s")
    [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$s.json" 2> /dev/null
    made=$(find "$WORK/raw/$s" -name '*.png' 2> /dev/null | head -1)
    if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $s: run ${n:-none}, exit $code: $(tail -c 300 "$OUT/$s.txt" | tr '\n' ' ')"; bad=1; continue; fi
    cp "$made" "$WORK/runs/$name/$image.png"
    say "$s: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, total \(((.settled_at_ms // 0) - (.created_at_ms // 0)) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB"' "$OUT/records/$s.json" 2> /dev/null)"
  done
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 4h -n 0 > "$OUT/machine.log" 2>&1
# The rental ends before the scoring, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

while read -r cand methods; do
  [ -n "$cand" ] || continue
  "$PY" -I "$FIX/colorfix.py" "$WORK/runs" "$cand" "$methods" >> "$OUT/post.txt" 2>&1 || { say "colorfix.py $cand: $(tail -c 300 "$OUT/post.txt")"; RC=1; }
done < <(jq -r '.[] | "\(.candidate) \(.methods)"' <<<"$POST")
"$PY" -I "$FIX/metrics.py" "$WORK/runs" --out "$OUT/scores.tsv" > "$OUT/scores.txt" 2>&1 || { say "metrics.py: $(tail -c 400 "$OUT/scores.txt")"; RC=1; }
"$PY" -I "$FIX/sheets.py" "$WORK/runs" --out "$WORK/sheets" > "$OUT/sheets.txt" 2>&1 || { say "sheets.py: $(tail -c 400 "$OUT/sheets.txt")"; RC=1; }
{ echo "## upscale on $NAME ($SKU)"; echo '```'; sed -n '/^candidate /,$p' "$OUT/scores.txt"; echo '```'; } > "$OUT/summary.md"
sed -n '/^candidate /,$p' "$OUT/scores.txt" | sed 's/^/  /' | tee -a "$R"
if [ "$(jq length <<<"$BARS")" -gt 0 ]; then
  "$PY" -I "$FIX/bars.py" "$OUT/scores.tsv" "$BARS" > "$OUT/bars.txt" 2>&1 || RC=1
  sed 's/^/  bar: /' "$OUT/bars.txt" | tee -a "$R"
  { echo '### bars'; echo '```'; cat "$OUT/bars.txt"; echo '```'; } >> "$OUT/summary.md"
fi
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
# What leaves the runner: the bench's outputs and sheets as they are, the private ones encrypted.
mkdir -p "$WORK/private"
for d in "$WORK"/runs/*/; do
  for f in "$d"*.png; do
    [ -f "$f" ] || continue
    image=$(basename "$f" .png) cand=$(basename "$d")
    if private "$image"; then mkdir -p "$WORK/private/runs/$cand" && cp "$f" "$WORK/private/runs/$cand/"; else mkdir -p "$OUT/runs/$cand" && cp "$f" "$OUT/runs/$cand/"; fi
  done
done
for f in "$WORK"/sheets/*; do
  [ -f "$f" ] || continue
  image=$(basename "$f"); image=${image%-*}
  if private "$image"; then mkdir -p "$WORK/private/sheets" && cp "$f" "$WORK/private/sheets/"; else cp "$f" "$OUT/sheets/"; fi
done
if [ -d "$WORK/private/runs" ]; then
  key=$(openssl rand -hex 32)
  tar cz -C "$WORK/private" . | openssl enc -aes-256-cbc -pbkdf2 -salt -pass "pass:$key" -out "$OUT/private.tar.gz.enc" &&
    printf %s "$key" | openssl pkeyutl -encrypt -pubin -inkey "$WORK/pub.pem" -out "$OUT/private.key.enc" || { say "the private outputs did not encrypt"; RC=1; }
  say "private: $(find "$WORK/private" -type f | wc -l) files, encrypted to the pubkey"
fi
[ "$bad" = 0 ] || RC=1
exit "$RC"
