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
# arm <label> <Runtime wheel>: the pod on that Runtime, then each setting undrafted and drafted (one run each, a fresh engine
# each), judged by the bar.
arm() {
  local label=$1 wheel=$2 set p d plain bad
  WANT=$(basename "$wheel" | cut -d- -f2)
  step "update-$label" machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$wheel" --tensorfs "$TF" -y || fail "$label: the pod's TensorD, Runtime and TensorFS"
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  say "$label on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name)"] | join(", "))' "$OUT/show-$label.json")"
  for set in "${SETTINGS[@]}"; do
    p=$OUT/plain-$label-$set.json d=$OUT/drafted-$label-$set.json
    step "plain-$label-$set" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "setting=$set" drafting:=false "arm=$label" --on "$NAME" ||
      fail "$label $set undrafted: $(tail -c 400 "$OUT/plain-$label-$set.txt")"
    record "plain-$label-$set" "$p"
    jq -e '.result.head' "$p" > /dev/null || fail "$label: the checkpoint has no draft head"
    if [ -n "$(jq -r '.result.error' "$p")" ]; then say "  $label $set: not run: $(jq -r '.result.error' "$p")"; continue; fi
    [[ $NOTE != *drafting* ]] || { say "FAIL $label $set: the undrafted call drafted: $NOTE"; RC=1; }
    plain=$(jq -c '.result.tokens' "$p")
    step "drafted-$label-$set" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "setting=$set" drafting:=true "against:=$plain" "arm=$label" --on "$NAME" ||
      fail "$label $set drafted: $(tail -c 400 "$OUT/drafted-$label-$set.txt")"
    record "drafted-$label-$set" "$d"
    [[ $NOTE == *drafting\ [0-9]* && $NOTE =~ ,\ [1-9][0-9]*\ captures ]] || { say "FAIL $label $set: the drafted call did not draft on recorded graphs: $NOTE"; RC=1; }
    say "  $label $set: $(jq -r '.result | "undrafted \($p[0].result.tokens | length), drafted \(.tokens | length) tokens, " + (if .same then "same" else "split at \(.first_difference), margin \(.margin * 1000 | round / 1000) (the teacher forward picks \(.teacher_picks))" end)' --slurpfile p "$p" "$d"); drafted: ${NOTE#*: }"
    bad=$(jq -r --argjson clear 0.5 '.result | select((.same | not) and ((if .margin < 0 then -.margin else .margin end) >= $clear)) | "split at \(.first_difference), margin \(.margin)"' "$d")
    [ -z "$bad" ] || { say "FAIL $label $set: a split at a clear position: $bad"; RC=1; }
    echo "- $label $set: $(jq -r '.result | if .same then "same" else "split at \(.first_difference), margin \(.margin)" end' "$d")" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
  done
}
arm candidate "$W"
[ -z "$CW" ] || arm control "$CW"
exit "${RC:-0}"
