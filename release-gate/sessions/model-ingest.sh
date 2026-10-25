# model-ingest: a first-party model onto tensorhub.com's root (xs-104). One rental:
#   1. uploads each source as its bf16 commit;
#   2. writes each lane from that commit by the recipe the package declares;
#   3. calls the package's function once per model on its first lane's commit (the gate);
#   4. publishes the release (bf16 + every lane), then calls each model once more through it.
# A failed first lane stops it; any other failure stops before publishing, and the commits stay private.
# packages_refs: one checkout (any name). params:
#   package    the packages directory whose recipes write the lanes and whose function the calls run
#   sources    space-separated name=source pairs; each source becomes the root model name's bf16 commit
#   lanes      comma-separated lanes the package's recipes write; the first one gates the release
#   version    the release: higher than every earlier one
#   traffic_gb GB the provider bills: the image, the sources and the uploads (430)
#   function (ask), slot (model): the call and its model slot
#   request    the calls' payload (JSON); a seed key gets a fresh seed per call
#   expect     text every gate answer contains (42; empty: none)
#   asset      the field that takes release-gate/ref/seed.jpg (images; empty: none)
. "$(dirname "$0")/lib.sh"
PACKAGE=$(param package) SOURCES=$(param sources) LANES=$(param lanes) VERSION=$(param version)
TRAFFIC_GB=$(param traffic_gb 430) FUNCTION=$(param function ask) SLOT=$(param slot model)
REQUEST=$(param request '{"prompt": "First, what is 17 + 25? Give just the number. Then describe this picture in two sentences.", "max_tokens": 160, "temperature": 0.7, "top_p": 0.8, "presence_penalty": 1.5, "seed": 0}')
EXPECT=$(param expect 42) ASSET=$(param asset images) IMG=$REF/seed.jpg
trees=("$PACKAGES"/*/)
[ "${#trees[@]}" = 1 ] && [ -d "${trees[0]}" ] || fail "packages_refs must name one checkout"
cd "${trees[0]}" || fail "no checkout"
made() { awk '$1=="commit"{c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/$1.txt"; }
lane_of() { cozy model info "$1" --json 2>/dev/null | jq -r --arg c "$2" '.commits[]? | select(.commit==$c) | .lane'; }
# call <name> <model ref>: the package's function once, with a fresh seed; passes on a completed run whose
# answers (texts) each contain EXPECT.
call() {
  local n=$1 ref=$2 N asset=()
  [ -z "$ASSET" ] || asset=(--asset "$ASSET=$IMG")
  jq --argjson s "$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')" 'if has("seed") then .seed = $s else . end' \
    <<<"$REQUEST" > "$OUT/$n.json" || return 1
  step "$n" run "./$PACKAGE/$FUNCTION" "model.$SLOT=$ref" --input "$OUT/$n.json" "${asset[@]}" --on "$NAME" || return 1
  N=$(runno "$n"); [ -n "$N" ] || return 1
  cozy run show "$N" --json > "$OUT/$n-run.json" 2>/dev/null
  say "$n run $N $(jq -r .status "$OUT/$n-run.json"): $(grep -E 'resolving|served' "$OUT/$n.txt" | tr -s ' ' | paste -sd';')"
  say "$n result: $(jq -c .result "$OUT/$n-run.json" | cut -c1-800)"
  jq -e --arg e "$EXPECT" '.status=="completed" and .result != null
    and ((.result.texts // [$e]) | all(ascii_downcase | contains($e | ascii_downcase)))' "$OUT/$n-run.json" >/dev/null
}

[ -d "$PACKAGE" ] || fail "packages has no $PACKAGE at $(git rev-parse --short HEAD)"
read -ra pairs <<<"$SOURCES"; IFS=, read -ra lanes <<<"$LANES"
[ "${#pairs[@]}" -gt 0 ] && [ "${#lanes[@]}" -gt 0 ] && [ -n "$VERSION" ] || fail "no sources, lanes or version"
for p in "${pairs[@]}"; do
  cozy model info "${p%%=*}" > "$OUT/before-${p%%=*}.txt" 2>&1
  grep -q model.not_found "$OUT/before-${p%%=*}.txt" || say "${p%%=*} exists on prod: $(head -2 "$OUT/before-${p%%=*}.txt" | paste -sd' ')"
done
printf 'commit   _/x@sha256:%064d\nrun 5 · x\nrun      17999\n' 7 > "$OUT/probe.txt"
[ "$(made probe)" = "sha256:$(printf '%064d' 7)" ] && [ "$(runno probe)" = 17999 ] || fail "parsers"
quote "$TRAFFIC_GB" 7200
rent

# 1. Each source as its bf16 commit.
declare -A BF16 LANE
for p in "${pairs[@]}"; do
  m=${p%%=*}
  step "upload-$m" model upload "${p#*=}" "$m" --lane bf16 --on "$NAME" || fail "upload $m"
  c=$(made "upload-$m"); [ "$(lane_of "$m" "$c")" = bf16 ] || fail "upload $m: no bf16 commit ($c)"
  BF16[$m]=$c; say "$m bf16 $c"
done
# 2. The lanes, by the package's recipes, from the bf16 commits.
failed=()
for lane in "${lanes[@]}"; do
  for p in "${pairs[@]}"; do
    m=${p%%=*}
    if step "$lane-$m" model quantize "$m@${BF16[$m]}" --lane "$lane" --package "./$PACKAGE" --on "$NAME" &&
       c=$(made "$lane-$m") && [ "$(lane_of "$m" "$c")" = "$lane" ]; then
      LANE[$m/$lane]=$c; say "$m $lane $c"
    else
      failed+=("$m/$lane"); say "LANE FAILED: $m $lane"
      [ "$lane" != "${lanes[0]}" ] || fail "$m's gating lane $lane was not written"
    fi
  done
done
# 3. The gate: each model answers on its first lane's commit.
for p in "${pairs[@]}"; do m=${p%%=*}; call "gate-$m" "$m@${LANE[$m/${lanes[0]}]}" || fail "$m did not pass on ${lanes[0]}"; done
[ "${#failed[@]}" = 0 ] || fail "not published: ${failed[*]} failed; every other commit is private on prod"
# 4. The release, then each model through it as this card resolves it.
for p in "${pairs[@]}"; do
  m=${p%%=*}; commits=("${BF16[$m]}")
  for lane in "${lanes[@]}"; do commits+=("${LANE[$m/$lane]}"); done
  cozy model publish "$m" "$VERSION" "${commits[@]}" > "$OUT/publish-$m.txt" 2>&1 || fail "publish $m: $(tail -3 "$OUT/publish-$m.txt")"
  sed 's/^/    /' "$OUT/publish-$m.txt" >> "$R"
  cozy model info "$m" > "$OUT/info-$m.txt" 2>&1
  say "PUBLISHED $m $VERSION"
done
for p in "${pairs[@]}"; do m=${p%%=*}; call "release-$m" "$m@$VERSION" || fail "$m@$VERSION did not pass through its release"; done
say "PASS: ${pairs[*]%%=*} released $VERSION (bf16 ${lanes[*]})"
