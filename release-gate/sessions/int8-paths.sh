# int8-paths (xs-213): where int8.convrot's one-pass forward and its token-by-token decode part on a ~1,000-token text
# prompt, on one rental: the decode drive's `int8_paths` (fixtures/ggml-decode/drive) with the int8 lane as `model` and
# the same checkpoint's bf16 lane as `reference`: each int8 path by row count on the same real rows bit for bit (GEMV,
# quantize + GEMM, eager, the fused norm prologue), then the 9B's forward against its decode on bf16, on int8 as it runs,
# on int8 with its decode given the prefill's kernel, and on the eager law, each against bf16's forward too.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime.
# `entries` picks the drive's calls in order (default int8_paths); paging_alone takes the reference alone (the pod's first
# call: bf16 resident), paging_beside both (bf16 paged beside int8).
# params: tensorfs, runtime (required), model, reference (lanes, org/name@sha256:…), entries, seconds (1200), skus and
#         quoted (preflight.sh pick_offer).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
MODEL=$(param model qwen/qwen3.5-9b@sha256:37a131b745e22e0c074592f349d6fd6778e5b127dd2a4b277e64ffd053742bd7)
REFERENCE=$(param reference qwen/qwen3.5-9b@sha256:f33bb62d3a36d385a3812bda4064b7e2881fcdc316e3d07ff5e7d86b66d10bdd)
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits
drive_lock "$FIX/drive"
pick_offer
quote "$(param seconds 1200)"
offer_rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "\(.name), driver \(.driver // "?")"] | join(", "))' "$OUT/show.json")"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
step pull-model model install "$MODEL" --on "$NAME" || fail "pull $MODEL"
step pull-reference model install "$REFERENCE" --on "$NAME" || fail "pull $REFERENCE"
FAILED=()
for entry in $(param entries int8_paths); do
  case $entry in
    paging_alone) models=(--model "model=$REFERENCE") ;;
    *) models=(--model "reference=$REFERENCE" --model "model=$MODEL") ;;
  esac
  step "$entry" run "$FIX/drive/$entry" "${models[@]}" --on "$NAME" ||
    { tail -c 3000 "$OUT/$entry.txt" | tee -a "$R"; FAILED+=("$entry"); continue; }
  n=$(runno "$entry"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$entry.json" 2> /dev/null
  jq -r '.result.detail // ""' "$OUT/$entry.json" > "$OUT/$entry-detail.json"
  say "$entry: $(head -c 6000 "$OUT/$entry-detail.json")"
done
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: measured (above)"
