# quantize-ab (xs-213, #3767's review item 2): one bf16 source quantized by `cozy model quantize --on local` on this runner, once on
# each arm's TensorFS (`a`, `b`: the RC's and a branch's), the same cozy and Runtime for both; every tensor of each lane compared
# between the arms by its stored parts' SHA-256 (fixtures/quantize-ab). No rental: it never quotes.
#   1. the source's bf16 commit installed on local once; a reader of the store (the `a` wheel) on the runner;
#   2. each arm: `machine update local` to its TensorFS and the Runtime, the package staged as its runs stage it (preflight.sh
#      pod_install), then each lane, its run's record naming the TensorFS that ran it (`measurements.software.tensorfs`): another
#      than the arm's candidate ends the session; each lane's commit digested at once (digest.py);
#   3. compare.py per lane: identical tensors, and where each quantized tensor's later parts were staged at the Runtime's
#      max_buffer_bytes (8 MiB): in memory or in scratch files.
# packages_refs: one checkout (`tree`). params:
#   tree      the packages_refs checkout (required)       package   its directory with the recipes (qwen3)
#   model     the bf16 source, org/name@release or org/name@sha256:… (huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0)
#   lanes     comma-separated recipe lanes (int8,nvfp4,w4a8)
#   a, b      each arm's TensorFS wheel source, as lib.sh `wheel_from` takes (required); `order` the arms' order ("a b")
#   runtime   the Runtime wheel source (default: RUNTIME_WHEEL, the dispatch's runtime_ref)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/quantize-ab
FLOOR_MBS=0  # no rented host whose link to judge
NAME=local
TREE=$(param tree '') PACKAGE=$(param package qwen3) MODEL=$(param model huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0)
LANES=$(param lanes int8,nvfp4,w4a8)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/$PACKAGE/pyproject.toml" ] || fail "no $PACKAGE in packages_refs checkout '$TREE'"
if [ -n "$(param runtime '')" ]; then
  RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") || RUNTIME=''
else
  RUNTIME=${RUNTIME_WHEEL:-}
fi
[ -f "$RUNTIME" ] || fail "no Runtime wheel: params runtime, or dispatch with runtime_ref"
declare -A TFS WANT
for arm in a b; do
  TFS[$arm]=$(wheel_from "$OUT/wheels/$arm" "$(param "$arm" '')") && [ -n "${TFS[$arm]}" ] || fail "no TensorFS wheel for arm $arm"
  WANT[$arm]=$(basename "${TFS[$arm]}" | cut -d- -f2)
done
say "cozy $(cozy --version) · runtime ${RUNTIME##*/} · a ${TFS[a]##*/} · b ${TFS[b]##*/} · $MODEL · lanes $LANES"
say "runner: $(nproc) CPUs, $(free -g | awk '/^Mem/{print $2}') GiB, $(df -h "$HOME" | awk 'NR==2{print $4}') free"
. "$HERE/fixtures/ggml-decode/preflight.sh"

# 1. The source, one commit for both arms.
if [[ $MODEL != *@sha256:* ]]; then
  cozy --json model show "$MODEL" --lane bf16 > "$OUT/source.json" 2> "$OUT/source.err" || fail "model show $MODEL: $(tail -c 300 "$OUT/source.err")"
  c=$(jq -r '.lanes[0].checkpoint // empty' "$OUT/source.json")
  [[ $c == sha256:* ]] || fail "$MODEL has no bf16 checkpoint: $(head -c 300 "$OUT/source.json")"
  MODEL="${MODEL%@*}@$c"
fi
say "source $MODEL"
step source model install "$MODEL" --on local || fail "install $MODEL on local"
READER=${RUNNER_TEMP:-$OUT}/quantize-ab-reader
uv venv -q -p 3.12 "$READER" && uv pip install -q -p "$READER" "${TFS[a]}" || fail "the store's reader"
STORE=${TENSORFS_HOME:-$HOME/.cache/tensorfs}
[ -d "$STORE" ] || fail "no TensorFS store at $STORE"
digest() {  # digest <label> <ref>: digest-<label>.json
  "$READER/bin/python" -I "$FIX/digest.py" "$STORE" "$MODEL" "$2" > "$OUT/digest-$1.json" 2> "$OUT/digest-$1.err" ||
    fail "digest $1 $2: $(tail -c 600 "$OUT/digest-$1.err")"
  say "$1: digested $(jq '.tensors | length' "$OUT/digest-$1.json") tensors of $2"
}

# 2. Each arm.
IFS=, read -ra lanes <<<"$LANES"
cd "$PACKAGES/$TREE" || fail "no checkout"
for arm in $(param order 'a b'); do
  [ -n "${TFS[$arm]:-}" ] || fail "order: no arm $arm"
  step "$arm-candidate" machine update local --tensorfs "${TFS[$arm]}" --runtime "$RUNTIME" -y || fail "$arm: local's TensorFS and Runtime"
  cozy --json machine show local > "$OUT/$arm-show.json" 2>&1
  got=$(jq -r '.status.tensorfs // empty' "$OUT/$arm-show.json")
  say "$arm: local runs $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs)"' "$OUT/$arm-show.json")"
  [ "$got" = "${WANT[$arm]}" ] || fail "$arm: local runs TensorFS '$got', not the candidate ${WANT[$arm]}"
  pod_install "$arm-stage" "$PACKAGES/$TREE/$PACKAGE"
  for lane in "${lanes[@]}"; do
    n=$arm-$lane
    step "$n" model quantize "$MODEL" --lane "$lane" --package "./$PACKAGE" --on local || fail "quantize $lane on arm $arm: $(grep -m1 '^error' "$OUT/$n.txt")"
    on_candidates "$n"
    c=$(made "$n"); [ -n "$c" ] || fail "$n made no checkpoint"
    (cd "$RUN_PROJECT" && cozy --json --full run watch "$(runno "$n")" --once) > "$OUT/$n-run.json" 2> "$OUT/$n-run.err"
    ran=$(jq -r '.measurements.software.tensorfs // empty' "$OUT/$n-run.json")
    say "$n: ${MODEL%@*}@$c · ran on $(jq -r '.measurements.software | "Runtime \(.runtime) · TensorFS \(.tensorfs) · TensorD \(.tensord)"' "$OUT/$n-run.json") ·" \
      "$(jq -r '"\(.execution_ms) ms executing · memoized \(.memoized) · transaction \(.result.transaction // "?")"' "$OUT/$n-run.json") ·" \
      "$(grep -c 'native quantization [0-9]*%' "$OUT/$n.txt") native quantization progress lines"
    [ "$ran" = "${WANT[$arm]}" ] || fail "$n ran on TensorFS '${ran:-unrecorded}', not arm $arm's candidate ${WANT[$arm]}"
    step "$n-held" model install "${MODEL%@*}@$c" --on local || fail "install ${MODEL%@*}@$c on local"
    digest "$n" "${MODEL%@*}@$c"
  done
done

# 3. The arms, lane by lane.
BAD=()
for lane in "${lanes[@]}"; do
  python3 -I "$FIX/compare.py" "$lane" "$OUT/digest-a-$lane.json" "$OUT/digest-b-$lane.json" > "$OUT/compare-$lane.json" 2> "$OUT/compare-$lane.err" ||
    BAD+=("$lane")
  cat "$OUT/compare-$lane.json" "$OUT/compare-$lane.err" | tee -a "$R"
done
say "store: $(du -sh "$STORE" | cut -f1); $(df -h "$HOME" | awk 'NR==2{print $4}') free"
[ "${#BAD[@]}" = 0 ] || fail "the arms' tensors differ: ${BAD[*]}"
say "PASS: ${LANES} byte for byte the same on TensorFS ${WANT[a]} and ${WANT[b]}"
