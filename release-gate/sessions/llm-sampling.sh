# llm-sampling (cr-346): a drafted decode keeps the tokens its undrafted one draws, sampled as vLLM samples, on one rental of $SKU.
# Paul, 11-18: "our runtime needs to support this for sure. Any sort of drafting / top-k 20 we need to support". The CPU cell
# (e2e/cli/llm.sh llm_sampling) shows the plain step's tokens are vLLM's draws; a CPU neither drafts nor records graphs. Here
# e2e/cli/sampler's `drafted` decodes Qwen3.5-9B (its MTP head) at one seed per setting: greedy with presence 1.5, Qwen-Image-2.1's
# t2i enhancer's (T 1, top-p 0.95, top-k 20, presence 1.5), and every setting (min-p, frequency and repetition too), first on a fresh
# engine with the head set aside, then drafting. Each pair must be the same tokens, and the drafted calls' notes must say they drafted on
# recorded graphs. The rental runs this cozy's TensorD and the change's Runtime (`runtime`). Without `rent`, $0: the quote and the checks before it.
# params:
#   runtime  the cozy-runtime dev-build run id of the Runtime the rental runs (required)
#   model    the checkpoint (default: `drafted`'s, qwen/qwen3.5-9b-mtp@0.2.0)
#   seconds  what the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SRC=$(param runtime '') MODEL=$(param model '')
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build of rc/<x>>"
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
say "runtime: ${W##*/}"
quote "$(param seconds 2400)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "the pod's TensorD and Runtime"
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show.json" 2> /dev/null
  [ "$(jq -r '.state // empty' "$OUT/show.json")" = ready ] && break
  sleep 5
done
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name)"] | join(", "))' "$OUT/show.json")"
step drafted run "$ROOT/e2e/cli/sampler/drafted" ${MODEL:+--model "model=$MODEL"} --on "$NAME" || fail "drafted: $(tail -c 400 "$OUT/drafted.txt")"
n=$(runno drafted)
(cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/drafted.json" 2> "$OUT/drafted.err" || fail "run $n's record"
jq -r '.result.pairs[] | "  \(.settings | tojson): plain \(.plain), drafted \(.drafted) tokens, same \(.same), first difference \(.first_difference)"' \
  "$OUT/drafted.json" | tee -a "$R"
jq -r '[.. | strings | select(test("tokens after .* ms/token"))] | .[]' "$OUT/drafted.json" | sed 's/^/  lm: /' | tee -a "$R"
jq -e '.result.head' "$OUT/drafted.json" > /dev/null || fail "the checkpoint has no draft head"
jq -e '.result.pairs | length == 4 and all(.same)' "$OUT/drafted.json" > /dev/null || { say "FAIL drafted: a drafted call's tokens are not its undrafted one's"; RC=1; }
# Each call is on a fresh engine, so it warms up eagerly first ("partly eager"); a drafted one says `drafting` and its captures.
[ "$(jq '[.. | strings | select(test("tokens after .*drafting [0-9].*, [1-9][0-9]* captures"))] | length' "$OUT/drafted.json")" -ge 4 ] ||
  { say "FAIL drafted: fewer than 4 calls drafted on recorded graphs"; RC=1; }
{ echo "## llm-sampling on $NAME ($SKU), run $n"; jq -r '.result.pairs[] | "- \(.settings | tojson): same \(.same)"' "$OUT/drafted.json"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "${RC:-0}"
