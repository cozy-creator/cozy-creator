# model-ingest: a first-party model onto tensorhub.com's root (xs-104). One rental:
#   1. uploads each source as its `from` commit (bf16);
#   2. pulls each `from` commit onto the pod, then writes each lane from it by the recipe the package declares;
#   3. calls the package's function once per model on its first lane's commit (the gate);
#   4. publishes the release (`from` + every lane), then calls each model once more through it.
# A failed first lane stops it; any other failure stops before publishing, and the commits stay private.
# packages_refs: one checkout (any name). params:
#   package    the packages directory whose recipes write the lanes and whose function the calls run
#   sources    space-separated name=source pairs; each source becomes the root model name's `from` commit
#   from       the lane the others are written from (bf16; fp8 for a model published only in fp8, as Ideogram 4)
#   have       space-separated name/lane=commit pairs already on the Hub (`from` too): not uploaded or written again
#   lanes      comma-separated lanes the package's recipes write; the first one gates the release
#   version    the release: higher than every earlier one
#   function (ask), slot (model): the call and its model slot
#   request    the calls' payload (JSON); a seed key gets a fresh seed per call
#   expect     text every gate answer contains (42; empty: none)
#   asset      the field that takes release-gate/ref/seed.jpg (images; empty: none)
#   gate       false: stop once the commits are written, with no call and no release, on a CPU rental whose
#              model needs a GPU to answer; a GPU session gates them and the release follows (true)
#   seconds    what the cap must buy (7200)
# Mirrors (xs-185): `mirrors`, a JSON array, puts authors' models and LoRAs on the Hub under their own orgs, as
# `cozy model upload --mirror` records them (admins), instead of the steps above. Each item:
#   model      org/name, under its author's org (required)   kind   model (default) or lora
#   page       the upstream page `--mirror` records (required)
#   uploads    [{source, lane, base?, base_lane?, format?, trained_on?, trigger?, strength?, commit?}]: each source a
#              commit in its lane, converted on the pod. `base` (a Hub model, release or checkpoint) supplies what the
#              source lacks (a DiT-only file's text encoder and VAE), in the upload's lane, or the release's `base_lane`
#              checkpoint. `commit` names one already made: a rerun skips it.
#   lanes      more lanes written from the bf16 commit by `recipes`' recipe (a Hub package, or ./<dir> of the checkout)
#   call       {function, request, slot?, strength?, lanes?}: one run on each commit (of `lanes` only, if given) before
#              publishing: a model as model.<slot>= (model), a LoRA as --lora <ref>,<strength> (1). ./<dir>/<fn> runs
#              the checkout's code, else the Hub's package. Outputs are in the session's artifact.
#   version    the release (required unless held)    tags, family   set with `model edit` / `lora edit`
#   hold       true: commits only, private, never published
# tensord: true makes this session's cozy (a dev build: `cozy` = its run id) the pod's TensorD first; runtime: true
# installs this session's RUNTIME_WHEEL (dispatched from rc/<x>, or runtime_ref) there too, for a package tree that
# needs a Runtime no release has. A Civitai source
# that needs an account reads CIVITAI_TOKEN, the release-gate secret pod-session.yaml gives Calcifer. A failed item
# stays private and the next goes on; the session fails if any did.
. "$(dirname "$0")/lib.sh"
PACKAGE=$(param package) SOURCES=$(param sources) LANES=$(param lanes) VERSION=$(param version) GIVEN=$(param have)
FUNCTION=$(param function ask) SLOT=$(param slot model)
REQUEST=$(param request '{"prompt": "First, what is 17 + 25? Give just the number. Then describe this picture in two sentences.", "max_tokens": 160, "temperature": 0.7, "top_p": 0.8, "presence_penalty": 1.5, "seed": 0}')
EXPECT=$(param expect 42) ASSET=$(param asset images) IMG=$REF/seed.jpg GATE=$(param gate true) FROM=$(param from bf16)
MIRRORS=$(param mirrors '')
printf 'checkpoint  sha256:%064d\ncheckpoint  _/x@sha256:%064d\nrun 5 · x\nrun      17999\n' 1 7 > "$OUT/probe.txt"
[ "$(made probe)" = "sha256:$(printf '%064d' 7)" ] && [ "$(runno probe)" = 17999 ] || fail "parsers"
trees=("$PACKAGES"/*/)
lane_of() { model_details "$1" 2>/dev/null | jq -r --arg c "$2" '.checkpoints[]? | select(.checkpoint==$c) | .lane'; }
# released <model> <release> <lane>: that release's checkpoint in that lane.
released() {
  model_details "$1" 2>/dev/null | jq -r --arg v "$2" --arg l "$3" '.releases[]? | select((.version // .release) == $v)
    | .lanes | (if type == "object" then to_entries | map({lane: .key} + .value) else . end) | .[] | select(.lane == $l) | .checkpoint'
}

# mirror <i>: mirrors[i] uploaded, called, tagged and published (unless held); 1 on its first failure.
mirror() {
  local it m kind verb page slug version strength j k v u lane c n N fn req pulled='' bf16='' commits=() calls=() args
  it=$(jq -c ".[$1]" <<<"$MIRRORS"); m=$(jq -r .model <<<"$it"); kind=$(jq -r '.kind // "model"' <<<"$it")
  page=$(jq -r .page <<<"$it"); version=$(jq -r '.version // ""' <<<"$it"); slug=${m//\//-}
  verb=model; [ "$kind" = model ] || verb=lora
  for ((j = 0; j < $(jq '.uploads | length' <<<"$it"); j++)); do
    u=$(jq -c ".uploads[$j]" <<<"$it"); lane=$(jq -r .lane <<<"$u"); c=$(jq -r '.commit // ""' <<<"$u")
    if [ -z "$c" ]; then
      args=("$(jq -r .source <<<"$u")" "$m" --lane "$lane" --mirror "$page")
      v=$(jq -r '.base // ""' <<<"$u")
      if [ -n "$v" ] && [ "$(jq -r '.base_lane // ""' <<<"$u")" != "" ]; then
        k=$(released "${v%@*}" "${v#*@}" "$(jq -r .base_lane <<<"$u")")
        [ -n "$k" ] || { say "MIRROR FAILED: $m: $v has no $(jq -r .base_lane <<<"$u") lane"; return 1; }
        v=${v%@*}@$k
      fi
      [ -z "$v" ] || args+=(--base "$v")
      for k in format:source-format trained_on:trained-on trigger:trigger strength:strength; do
        v=$(jq -r --arg k "${k%%:*}" '.[$k] // "" | tostring' <<<"$u")
        [ -z "$v" ] || args+=("--${k#*:}" "$v")
      done
      # A failed upload is asked once more: what reached the Hub stays there, and the second sends the rest.
      if step "upload-$slug-$j" "$verb" upload "${args[@]}" --on "$NAME"; then c=$(made "upload-$slug-$j")
      else
        say "$m: upload $j ($lane) failed: $(tail -2 "$OUT/upload-$slug-$j.txt" | paste -sd' '); asking again"
        step "upload-$slug-$j-again" "$verb" upload "${args[@]}" --on "$NAME" || { say "MIRROR FAILED: $m: upload $j ($lane)"; return 1; }
        c=$(made "upload-$slug-$j-again")
      fi
    fi
    [ "$(lane_of "$m" "$c")" = "$lane" ] || { say "MIRROR FAILED: $m: ${c:-no commit} is no $lane commit"; return 1; }
    commits+=("$c"); calls+=("$lane=$c"); [ "$lane" != bf16 ] || bf16=$c
    say "COMMIT $m $lane $c"
  done
  for lane in $(jq -r '.lanes // [] | .[]' <<<"$it"); do
    v=$(jq -r '.recipes // ""' <<<"$it"); [[ $v != ./* ]] || v=$TREE/${v#./}
    [ -n "$bf16" ] && [ -n "$v" ] || { say "MIRROR FAILED: $m: lane $lane needs a bf16 upload and recipes"; return 1; }
    [ -n "$pulled" ] || step "pull-$slug" model install "$m@$bf16" --on "$NAME" || { say "MIRROR FAILED: $m: pull"; return 1; }
    pulled=1
    step "$lane-$slug" model quantize "$m@$bf16" --lane "$lane" --package "$v" --on "$NAME" &&
      c=$(made "$lane-$slug") && [ "$(lane_of "$m" "$c")" = "$lane" ] || { say "MIRROR FAILED: $m: lane $lane"; return 1; }
    commits+=("$c"); calls+=("$lane=$c"); say "COMMIT $m $lane $c"
  done
  fn=$(jq -r '.call.function // ""' <<<"$it")
  for v in "${calls[@]}"; do
    [ -n "$fn" ] || break
    lane=${v%%=*} c=${v#*=}
    jq -e --arg l "$lane" '(.call.lanes // null) == null or (.call.lanes | index($l))' <<<"$it" >/dev/null || continue
    n=call-$slug-$lane; req=$OUT/$n.json
    jq --argjson s "$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')" '.call.request // {} | if has("seed") then .seed = $s else . end' \
      <<<"$it" > "$req" || return 1
    args=(run "$fn" --input "$req" -o "$OUT/$n" --on "$NAME")
    if [ "$kind" = model ]; then args+=("model.$(jq -r '.call.slot // "model"' <<<"$it")=$m@$c")
    else args+=(--lora "$m@$c,$(jq -r '.call.strength // 1' <<<"$it")"); fi
    step "$n" "${args[@]}"; N=$(runno "$n")
    [ -n "$N" ] && cozy run watch "$N" --json --full > "$OUT/$n-run.json" 2>/dev/null
    say "$n run ${N:-none} $(jq -r .status "$OUT/$n-run.json" 2>/dev/null): $(jq -c .result "$OUT/$n-run.json" 2>/dev/null | cut -c1-300)"
    jq -e '.status == "completed" and .result != null' "$OUT/$n-run.json" >/dev/null 2>&1 ||
      { say "MIRROR FAILED: $m: its $lane commit's call"; return 1; }
  done
  args=()
  for v in $(jq -r '.tags // [] | .[]' <<<"$it"); do args+=(--tag "$v"); done
  v=$(jq -r '.family // ""' <<<"$it"); [ -z "$v" ] || args+=(--family "$v")
  if [ "${#args[@]}" -gt 0 ]; then
    cozy "$verb" edit "$m" "${args[@]}" > "$OUT/edit-$slug.txt" 2>&1 || { say "MIRROR FAILED: $m: edit $(tail -2 "$OUT/edit-$slug.txt")"; return 1; }
  fi
  if [ "$(jq -r '.hold // false' <<<"$it")" = true ]; then
    say "HELD $m: ${commits[*]} (private, not published)"; return 0
  fi
  cozy "$verb" publish "$m" "$version" "${commits[@]}" > "$OUT/publish-$slug.txt" 2>&1 ||
    { say "MIRROR FAILED: $m: publish $(tail -3 "$OUT/publish-$slug.txt")"; return 1; }
  model_details "$m" > "$OUT/info-$slug.json" 2>&1
  say "PUBLISHED $m $version: ${calls[*]}"
}

if [ -n "$MIRRORS" ]; then
  TREE=''
  if [ -d "${trees[0]}" ]; then
    [ "${#trees[@]}" = 1 ] || fail "packages_refs must name one checkout"
    TREE=${trees[0]%/}; cd "$TREE" || fail "no checkout"
  fi
  n=$(jq -e 'if type == "array" then length else error("not an array") end' <<<"$MIRRORS") || fail "mirrors is not a JSON array"
  for ((i = 0; i < n; i++)); do
    jq -e --argjson i "$i" '.[$i] | (.model | type == "string" and test("^[a-z0-9][a-z0-9_.-]*/[a-z0-9][a-z0-9_.-]*$"))
      and (.page | type == "string" and length > 0) and ((.kind // "model") | IN("model", "lora"))
      and (.uploads | type == "array" and length > 0) and all(.uploads[]; (.source // .commit | type == "string") and (.lane | type == "string"))
      and (.hold == true or (.version | type == "string"))' <<<"$MIRRORS" >/dev/null ||
      fail "mirrors[$i] needs model (org/name), page, kind (model or lora), uploads [{source, lane}] and a version unless held"
    m=$(jq -r ".[$i].model" <<<"$MIRRORS"); [[ $m != fidika/* ]] || fail "$m: a mirror lives under its author's org, never fidika"
    model_details "$m" > "$OUT/before-${m//\//-}.txt" 2>&1
    grep -q model.not_found "$OUT/before-${m//\//-}.txt" || say "$m exists on prod: $(head -c 200 "$OUT/before-${m//\//-}.txt")"
  done
  quote "$(param seconds 7200)"
  rent
  if [ "$(param tensord false)" = true ]; then
    step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
    cozy machine show "$NAME" > "$OUT/show-update.txt" 2>&1; grep -E '^(version|runtime|tensorfs) ' "$OUT/show-update.txt" | tee -a "$R"
  fi
  if [ "$(param runtime false)" = true ]; then
    [ -n "${RUNTIME_WHEEL:-}" ] || fail "runtime: true, and this session has no RUNTIME_WHEEL (dispatch from rc/<x>, or runtime_ref)"
    step runtime machine update "$NAME" --runtime "$RUNTIME_WHEEL" -y || fail "the pod's Runtime"
    cozy machine show "$NAME" 2>&1 | grep -E '^runtime ' | tee -a "$R"
  fi
  failed=()
  for ((i = 0; i < n; i++)); do mirror "$i" || failed+=("$(jq -r ".[$i].model" <<<"$MIRRORS")"); done
  [ "${#failed[@]}" = 0 ] || fail "not mirrored: ${failed[*]}; their commits stay private"
  say "PASS: $n mirrored"
  exit 0
fi
[ "${#trees[@]}" = 1 ] && [ -d "${trees[0]}" ] || fail "packages_refs must name one checkout"
cd "${trees[0]}" || fail "no checkout"
# call <name> <model ref>: the package's function once, with a fresh seed; passes on a completed run whose
# answers (texts) each contain EXPECT.
call() {
  local n=$1 ref=$2 N asset=()
  [ -z "$ASSET" ] || asset=(--file "$ASSET=$IMG")
  jq --argjson s "$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')" 'if has("seed") then .seed = $s else . end' \
    <<<"$REQUEST" > "$OUT/$n.json" || return 1
  step "$n" run "./$PACKAGE/$FUNCTION" "model.$SLOT=$ref" --input "$OUT/$n.json" "${asset[@]}" --on "$NAME" || return 1
  N=$(runno "$n"); [ -n "$N" ] || return 1
  cozy run watch "$N" --json --full > "$OUT/$n-run.json" 2>/dev/null
  say "$n run $N $(jq -r .status "$OUT/$n-run.json"): $(grep -E 'resolving|served' "$OUT/$n.txt" | tr -s ' ' | paste -sd';')"
  say "$n result: $(jq -c .result "$OUT/$n-run.json" | cut -c1-800)"
  jq -e --arg e "$EXPECT" '.status=="completed" and .result != null
    and ((.result.texts // [$e]) | all(ascii_downcase | contains($e | ascii_downcase)))' "$OUT/$n-run.json" >/dev/null
}

[ -d "$PACKAGE" ] || fail "packages has no $PACKAGE at $(git rev-parse --short HEAD)"
read -ra pairs <<<"$SOURCES"; IFS=, read -ra lanes <<<"$LANES"
[ "${#pairs[@]}" -gt 0 ] && [ "${#lanes[@]}" -gt 0 ] && [ -n "$VERSION" ] || fail "no sources, lanes or version"
for p in "${pairs[@]}"; do
  m=${p%%=*}; model_details "$m" > "$OUT/before-${m//\//-}.txt" 2>&1
  grep -q model.not_found "$OUT/before-${m//\//-}.txt" || say "$m exists on prod: $(head -2 "$OUT/before-${m//\//-}.txt" | paste -sd' ')"
done
declare -A SOURCE LANE
for p in $GIVEN; do
  k=${p%%=*} c=${p#*=}; m=${k%/*} l=${k##*/}
  [ "$(lane_of "$m" "$c")" = "$l" ] || fail "have: $m has no $l commit $c"
  if [ "$l" = "$FROM" ]; then SOURCE[$m]=$c; else LANE[$m/$l]=$c; fi
  say "$m $l $c (have)"
done
quote "$(param seconds 7200)"
rent

# 1. Each source as its $FROM commit, unless given.
for p in "${pairs[@]}"; do
  m=${p%%=*}
  [ -z "${SOURCE[$m]:-}" ] || continue
  step "upload-${m//\//-}" model upload "${p#*=}" "$m" --lane "$FROM" --on "$NAME" || fail "upload $m"
  c=$(made "upload-${m//\//-}"); [ "$(lane_of "$m" "$c")" = "$FROM" ] || fail "upload $m: no $FROM commit ($c)"
  SOURCE[$m]=$c; say "$m $FROM $c"
done
# 2. The lanes, by the package's recipes, from the $FROM commits, each pulled whole first: a quantize then reads its
# source from the pod's store, not tensor by tensor from the Hub.
for p in "${pairs[@]}"; do
  m=${p%%=*}
  for lane in "${lanes[@]}"; do
    [ -n "${LANE[$m/$lane]:-}" ] && continue
    step "pull-${m//\//-}" model install "$m@${SOURCE[$m]}" --on "$NAME" || fail "pull $m's $FROM commit"
    break
  done
done
failed=()
for lane in "${lanes[@]}"; do
  for p in "${pairs[@]}"; do
    m=${p%%=*}
    [ -z "${LANE[$m/$lane]:-}" ] || continue
    if step "$lane-${m//\//-}" model quantize "$m@${SOURCE[$m]}" --lane "$lane" --package "./$PACKAGE" --on "$NAME" &&
       c=$(made "$lane-${m//\//-}") && [ "$(lane_of "$m" "$c")" = "$lane" ]; then
      LANE[$m/$lane]=$c; say "$m $lane $c"
    else
      failed+=("$m/$lane"); say "LANE FAILED: $m $lane"
      [ "$lane" != "${lanes[0]}" ] || fail "$m's gating lane $lane was not written"
    fi
  done
done
if [ "$GATE" != true ]; then
  [ "${#failed[@]}" = 0 ] || fail "not written: ${failed[*]}; every other commit is private on prod"
  for p in "${pairs[@]}"; do m=${p%%=*}; for lane in "$FROM" "${lanes[@]}"; do
    c=${SOURCE[$m]}; [ "$lane" = "$FROM" ] || c=${LANE[$m/$lane]}; say "COMMIT $m $lane $c"
  done; done
  say "PASS: ${pairs[*]%%=*} written ($FROM ${lanes[*]}); not gated, not released"
  exit 0
fi
# 3. The gate: each model answers on its first lane's commit.
for p in "${pairs[@]}"; do m=${p%%=*}; call "gate-${m//\//-}" "$m@${LANE[$m/${lanes[0]}]}" || fail "$m did not pass on ${lanes[0]}"; done
[ "${#failed[@]}" = 0 ] || fail "not published: ${failed[*]} failed; every other commit is private on prod"
# 4. The release, then each model through it as this card resolves it.
for p in "${pairs[@]}"; do
  m=${p%%=*}; commits=("${SOURCE[$m]}")
  for lane in "${lanes[@]}"; do commits+=("${LANE[$m/$lane]}"); done
  cozy model publish "$m" "$VERSION" "${commits[@]}" > "$OUT/publish-${m//\//-}.txt" 2>&1 || fail "publish $m: $(tail -3 "$OUT/publish-${m//\//-}.txt")"
  sed 's/^/    /' "$OUT/publish-${m//\//-}.txt" >> "$R"
  model_details "$m" > "$OUT/info-${m//\//-}.txt" 2>&1
  say "PUBLISHED $m $VERSION"
done
for p in "${pairs[@]}"; do m=${p%%=*}; call "release-${m//\//-}" "$m@$VERSION" || fail "$m@$VERSION did not pass through its release"; done
say "PASS: ${pairs[*]%%=*} released $VERSION ($FROM ${lanes[*]})"
