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
# arm <label> <Runtime wheel>: the pod on that Runtime, `drafted` on it, its record judged by the bar.
arm() {
  local label=$1 wheel=$2 n f=$OUT/drafted-$1.json ran want bad
  step "update-$label" machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$wheel" --tensorfs "$TF" -y || fail "$label: the pod's TensorD, Runtime and TensorFS"
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  say "$label on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name)"] | join(", "))' "$OUT/show-$label.json")"
  step "drafted-$label" run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} "arm=$label" --on "$NAME" || fail "$label: $(tail -c 400 "$OUT/drafted-$label.txt")"
  n=$(runno "drafted-$label")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$f" 2> "$OUT/drafted-$label.err" || fail "$label: run $n's record"
  [ "$(jq -r '.memoized // false' "$f")" != true ] || fail "$label: run $n was answered from the memo; it measured nothing"
  ran=$(jq -r '.result.runtime // empty' "$f") want=$(basename "$wheel" | cut -d- -f2)
  [ "$ran" = "$want" ] || fail "$label: the run's Runtime is $ran, not $want"
  jq -e '.result.head' "$f" > /dev/null || fail "$label: the checkpoint has no draft head"
  jq -r --arg l "$label" '.result.pairs[] | "  \($l) \(.name): " + (if .error != "" then "not run: \(.error)" else
    "plain \(.plain), drafted \(.drafted) tokens, same \(.same)" + (if .same then "" else ", split at \(.first_difference), margin \(.margin * 1000 | round / 1000) (the teacher forward picks \(.teacher_picks))" end)
    + "; engines plain \(.plain_engine | tojson), drafted \(.drafted_engine | tojson)" end)' "$f" | tee -a "$R"
  bad=$(jq -c --argjson clear 0.5 '[.result.pairs[] | select(.error == "" and (.same | not) and ((if .margin < 0 then -.margin else .margin end) >= $clear)) | {name, first_difference, margin}]' "$f")
  [ "$bad" = '[]' ] || { say "FAIL $label: a split at a clear position: $bad"; RC=1; }
  bad=$(jq -c '[.result.pairs[] | select(.error == "" and (.drafted_engine.drafting_graphs < 1 or .drafted_engine.replays < 1 or .plain_engine.drafting_graphs > 0)) | .name]' "$f")
  [ "$bad" = '[]' ] || { say "FAIL $label: drafted calls that recorded no drafting graph or replayed none, or plain ones that drafted: $bad"; RC=1; }
  { echo "## llm-sampling $label on $NAME ($SKU), run $n"
    jq -r '.result.pairs[] | "- \(.name): " + (if .error != "" then "not run" elif .same then "same" else "split at \(.first_difference), margin \(.margin)" end)' "$f"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
}
arm candidate "$W"
[ -z "$CW" ] || arm control "$CW"
exit "${RC:-0}"
