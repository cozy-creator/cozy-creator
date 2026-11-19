# llm-sampling (cr-346): a drafted decode against its undrafted one, sampled as vLLM samples, on one rental of $SKU.
# Paul, 11-18: "our runtime needs to support this for sure. Any sort of drafting / top-k 20 we need to support". The CPU cell
# (e2e/cli/llm.sh llm_sampling) shows the plain step's tokens are vLLM's draws; a CPU neither drafts nor records graphs. Here
# e2e/cli/sampler's `drafted` decodes Qwen3.5-9B (its MTP head) at one seed per setting: greedy without and with presence 1.5,
# Qwen-Image-2.1's t2i enhancer's (T 1, top-p 0.95, top-k 20, presence 1.5), a top-k that binds (top-k 5, presence 1.5) and every
# setting (min-p, frequency and repetition too), first on a fresh engine with the head set aside, then drafting. The bar (the
# coordinator, 11-19): both choose the same token at every clear position; a split is allowed only at a near tie, its margin under
# 0.5 (logits; z + Gumbel when sampled; `split_margin`), and is reported. Each drafted call must have drafted on recorded graphs
# that replayed, each undrafted one not. `control`, a Runtime run id: the same on that Runtime after (the RC's, for the greedy
# pairs it takes). The rental runs this cozy's TensorD and the change's Runtime (`runtime`) and TensorFS (`tensorfs`). Without
# `rent`, $0: the real lock and the quote.
# params:
#   runtime  the cozy-runtime dev-build run id of the Runtime the rental runs (required)
#   tensorfs the TensorFS wheel beside it (an https URL or artifact:<run>/<name>; required)
#   control  a cozy-runtime dev-build run id whose Runtime runs `drafted` after (optional)
#   model    the checkpoint (default: `drafted`'s, qwen/qwen3.5-9b@0.2.0)
#   only     drafted, think or both, comma-separated (default drafted)
#   seconds  what the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SRC=$(param runtime '') TFS=$(param tensorfs '') CONTROL=$(param control '') MODEL=$(param model '')
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build of rc/<x>>"
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
[ -n "$TFS" ] && TF=$(wheel_from "$OUT/tensorfs" "$TFS") && [ -n "$TF" ] || fail "no TensorFS wheel from '$TFS'"
say "runtime: ${W##*/}, tensorfs: ${TF##*/}"
CW=
if [ -n "$CONTROL" ]; then
  [[ $CONTROL =~ ^[0-9]+$ ]] && CW=$(wheel_from "$OUT/control" "$CONTROL") && [ -n "$CW" ] || fail "no control wheel in cozy-runtime run $CONTROL"
  say "control: ${CW##*/}"
fi
# The real lock at $0: Calcifer locks the proof package as a run stages it, so a lock uv can't make fails here, before a rental.
step lock package install "$ROOT/e2e/cli/sampler" --on local || fail "the proof package doesn't lock: $(tail -c 400 "$OUT/lock.txt")"
cozy package ls > "$OUT/packages.txt" 2>&1; grep -i sampler "$OUT/packages.txt" | sed 's/^/  lock: /' | tee -a "$R"
quote "$(param seconds 2400)"
rent
# record <step> <file>: the run that step named, its whole record; fails on a memo's answer, which measured nothing.
record() {
  local n; n=$(runno "$1")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$2" 2> "$2.err" || fail "$1: run $n's record"
  [ "$(jq -r '.memoized // false' "$2")" != true ] || fail "$1: run $n was answered from the memo; it measured nothing"
  [ "$(jq -r '.result.runtime // empty' "$2")" = "$WANT" ] || fail "$1: run $n ran Runtime $(jq -r '.result.runtime // "?"' "$2"), not $WANT"
  NOTE=$(jq -r '[.. | strings | select(test("tokens after .* ms/token"))] | last // ""' "$2")
}
SETTINGS=(greedy greedy-presence recipe order knobs)
# split <file>: a judged call's line: the same tokens, or where it split from the reference, the least logit change that turns it
# (`split_margin`), which token a teacher-forced forward picks, and the parts.
split() {
  jq -r '.result | if .same then "same" else "split at \(.first_difference), margin \(.margin * 1000 | round / 1000) (\(if (if .margin < 0 then -.margin else .margin end) < 0.5 then "near tie" else "CLEAR" end); the teacher forward picks \(.teacher_picks); parts \(.parts | map_values(. * 1000 | round / 1000) | tojson))" end' "$1"
}
clear() { jq -e '.result | (.same | not) and ((if .margin < 0 then -.margin else .margin end) >= 0.5)' "$1" > /dev/null; }
# arm <label> <Runtime wheel>: the pod on that Runtime; the forward run twice (`repeat`: where it first differs); then each
# setting undrafted (the reference), undrafted again (the noise floor) and drafted, each its own run on a fresh engine, the last
# two judged against the first. The bug's signature (the coordinator, 11-19): a drafted split clear on every part while no
# undrafted repeat ever splits clear.
arm() {
  local label=$1 wheel=$2 set p q d ref floor=0 bugs=()
  WANT=$(basename "$wheel" | cut -d- -f2)
  step "update-$label" machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$wheel" --tensorfs "$TF" -y || fail "$label: the pod's TensorD, Runtime and TensorFS"
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  say "$label on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name)"] | join(", "))' "$OUT/show-$label.json")"
  step "repeat-$label" run "$ROOT/e2e/cli/sampler/repeat" ${MODEL:+--model "model=$MODEL"} "arm=$label" --on "$NAME" || fail "$label repeat: $(tail -c 400 "$OUT/repeat-$label.txt")"
  record "repeat-$label" "$OUT/repeat-$label.json"
  say "  $label repeat: $(jq -r '.result | "logits differ by \(.logits_max_diff) between two identical forwards; first differing module \(.first_module // "none") by \(.first_diff); \(.differing) of \(.watched) watched modules differ"' "$OUT/repeat-$label.json")"
  for set in "${SETTINGS[@]}"; do
    p=$OUT/plain-$label-$set.json q=$OUT/again-$label-$set.json d=$OUT/drafted-$label-$set.json
    step "plain-$label-$set" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "setting=$set" drafting:=false "arm=$label" --on "$NAME" ||
      fail "$label $set undrafted: $(tail -c 400 "$OUT/plain-$label-$set.txt")"
    record "plain-$label-$set" "$p"
    jq -e '.result.head' "$p" > /dev/null || fail "$label: the checkpoint has no draft head"
    if [ -n "$(jq -r '.result.error' "$p")" ]; then say "  $label $set: not run: $(jq -r '.result.error' "$p")"; continue; fi
    [[ $NOTE != *drafting* ]] || { say "FAIL $label $set: the undrafted call drafted: $NOTE"; RC=1; }
    ref=$(jq -c '.result.tokens' "$p")
    step "again-$label-$set" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "setting=$set" drafting:=false "against:=$ref" "arm=$label-again" --on "$NAME" ||
      fail "$label $set undrafted again: $(tail -c 400 "$OUT/again-$label-$set.txt")"
    record "again-$label-$set" "$q"
    [[ $NOTE != *drafting* ]] || { say "FAIL $label $set: the undrafted repeat drafted: $NOTE"; RC=1; }
    step "drafted-$label-$set" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "setting=$set" drafting:=true "against:=$ref" "arm=$label" --on "$NAME" ||
      fail "$label $set drafted: $(tail -c 400 "$OUT/drafted-$label-$set.txt")"
    record "drafted-$label-$set" "$d"
    [[ $NOTE == *drafting\ [0-9]* && $NOTE =~ ,\ [1-9][0-9]*\ captures ]] || { say "FAIL $label $set: the drafted call did not draft on recorded graphs: $NOTE"; RC=1; }
    say "  $label $set ($(jq '.result.tokens | length' "$p") tokens): undrafted again $(split "$q")"
    say "  $label $set: drafted $(split "$d"); ${NOTE#*: }"
    ! clear "$q" || floor=$((floor + 1))
    ! clear "$d" || bugs+=("$set")
    echo "- $label $set: again $(jq -r '.result | if .same then "same" else "split \(.first_difference) margin \(.margin)" end' "$q"), drafted $(jq -r '.result | if .same then "same" else "split \(.first_difference) margin \(.margin)" end' "$d")" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
  done
  if [ "${#bugs[@]}" -gt 0 ] && [ "$floor" = 0 ]; then
    say "FAIL $label: the bug's signature: drafted splits clear on every part (${bugs[*]}) while no undrafted repeat split clear"; RC=1
  elif [ "${#bugs[@]}" -gt 0 ]; then
    say "  $label: drafted splits clear by this measure (${bugs[*]}), and $floor undrafted repeats split clear too: the measure doesn't separate them; no bug's signature"
  else
    say "  $label: every drafted split is a near tie; $floor undrafted repeats split clear"
  fi
}
# think (cr-351, `only` holds think): Qwen3.5-9B as its card sets it (the general thinking profile, up to 32,768 new tokens), its
# answer held to a JSON schema after the thought (`Params.allowed`, the engine's `After(</think>)` its own), at three seeds, one
# run each: each must close its thought and parse exactly. First the grammar's own cost, which a short answer can't show: ten fruits without
# thinking, held or free, drafted and not (each run's decode note: ms/token); then the thinking calls with and without it.
think() {
  local c d f
  for c in true false; do
    for d in true false; do
      f=$OUT/cost-$c-$d.json
      step "cost-$c-$d" run "$ROOT/e2e/cli/sampler/schema_cost" ${MODEL:+--model "model=$MODEL"} constrained:=$c drafting:=$d "arm=cost-$c-$d" --on "$NAME" ||
        fail "cost $c $d: $(tail -c 400 "$OUT/cost-$c-$d.txt")"
      record "cost-$c-$d" "$f"
      say "  schema cost constrained=$c drafting=$d: $(jq -r '.result | "parsed \(.parsed), \(.tokens) tokens, grammar \(.grammar_ms) ms/token"' "$f"); ${NOTE#*: }"
      [ "$c" = false ] || jq -e '.result.parsed' "$f" > /dev/null || { say "FAIL cost: the held answer doesn't parse: $(jq -c '.result.answer' "$f")"; RC=1; }
    done
  done
  local seed
  for seed in 7 8 9; do  # think_then_schema's THOUGHT_SEEDS, each arm's first before any second: a cap that ends the pod leaves every arm measured
    for c in true false; do
      for d in true false; do
        f=$OUT/think-$c-$d-$seed.json
        step "think-$c-$d-$seed" run "$ROOT/e2e/cli/sampler/think_then_schema" ${MODEL:+--model "model=$MODEL"} constrained:=$c drafting:=$d seed:=$seed "arm=think-$c-$d" --on "$NAME" ||
          fail "think $c $d $seed: $(tail -c 400 "$OUT/think-$c-$d-$seed.txt")"
        record "think-$c-$d-$seed" "$f"
        say "  think constrained=$c drafting=$d seed $seed: $(jq -r '.result | "closed \(.closed), parsed \(.parsed), thought \(.thought_tokens) + answer \(.answer_tokens) tokens in \(.seconds) s, grammar \(.grammar_ms) ms/token"' "$f"); ${NOTE#*: }"
        if [ "$c" = true ]; then
          jq -e '.result | .closed and .parsed' "$f" > /dev/null || {
            say "FAIL think: seed $seed didn't close its thought and parse: $(jq -c '.result | {thought_tail: .thought_tail[-200:], answer: .answer[-200:]}' "$f")"; RC=1; }
        fi
      done
    done
  done
}
ONLY=$(param only drafted)
WANT=$(basename "$W" | cut -d- -f2)
[[ ,$ONLY, != *,drafted,* ]] || arm candidate "$W"
[[ ,$ONLY, != *,drafted,* ]] || [ -z "$CW" ] || arm control "$CW"
if [[ ,$ONLY, == *,think,* ]]; then
  step update-think machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" --tensorfs "$TF" -y || fail "think: the candidate Runtime"
  WANT=$(basename "$W" | cut -d- -f2)
  think
fi
exit "${RC:-0}"
