# engine-routes (xs-213 phase 2): with CozyEngine switched on (a Runtime whose `vlm.COZY_ENGINE` is True), every route a
# `qwen3/ask` call takes, through `cozy run` on one rental of $SKU, each read from the run's own `.engine` note:
#   served    the q8_0 lane, text: the engine serves it ("by the engine");
#   picture   the same lane with a picture: Transformers' engine ("reads text only");
#   unknown   the int8.convrot lane: Transformers' engine (a lane the engine's cells have not passed);
#   rows      17 questions in one `VisionLM.ask` (the decode drive's `rows`; AskRequest takes 16): Transformers' engine;
#   room      the q8_0 lane with the GPU memory limited to `capped`: its weights' lease is refused at the call's start, so
#             Transformers' engine serves it ("don't fit at the call's start").
# Every call must answer with words. The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime.
# params: tree (required), tensorfs, runtime (required), capped (6GiB), seconds (1800), skus and quoted (preflight.sh).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
TREE=$(param tree '') CAPPED=$(param capped 6GiB)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
[[ $CAPPED =~ ^[0-9]+[MG]iB$ ]] || fail "capped: <n>GiB"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
Q8=qwen/qwen3.5-9b@sha256:c1059b7b191259ecd579df4640b6abd4e419aa25761135f0bd12c8e4b040393c
INT8=qwen/qwen3.5-9b@sha256:37a131b745e22e0c074592f349d6fd6778e5b127dd2a4b277e64ffd053742bd7
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
kernels "$SOURCE/qwen3"
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits "qwen3=$SOURCE/qwen3/uv.lock"
drive_lock "$FIX/drive"
pick_offer
quote "$(param seconds 1800)"
offer_rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | .name] | join(", "))' "$OUT/show.json")"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"

record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
# note <step>: the run's `.engine` note (which engine served it, and why not the engine).
note() { jq -r '[.. | objects | to_entries[] | select((.key | endswith(".engine")) and (.value | type == "string")) | .value] | first // ""' "$OUT/$1.json" 2> /dev/null; }
# route <step> <the note's words> <run args...>: the call answers with words, and its note says the route.
route() {
  local n=$1 want=$2 said heard; shift 2
  step "$n" run "$@" --on "$NAME" || { tail -c 2000 "$OUT/$n.txt" | tee -a "$R"; FAILED+=("$n"); return; }
  record "$n"
  heard=$(note "$n")
  said=$(jq -r '.result.texts[0] // .result.detail // ""' "$OUT/$n.json" 2> /dev/null)
  say "$n: engine note: ${heard:-none} · answer: $(head -c 300 <<<"$said")"
  [[ $heard == *"$want"* ]] || { say "FAILED: $n: its note doesn't say '$want'"; FAILED+=("$n"); return; }
  [ -n "${said//[[:space:]]/}" ] || { say "FAILED: $n: no words"; FAILED+=("$n"); }
}
STORY='Write a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep writing until you are stopped.'
FAILED=()
step pull-q8 model install "$Q8" --on "$NAME" || fail "pull q8_0"
step pull-int8 model install "$INT8" --on "$NAME" || fail "pull int8"
route served "by the engine" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=64 temperature:=0
route picture "reads text only" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=What is in this picture?" \
  --file "images=$REF/seed.jpg" max_tokens:=32 temperature:=0
route unknown "int8.convrot" "$SOURCE/qwen3/ask" --model "model=$INT8" "prompt=$STORY" max_tokens:=32 temperature:=0
route rows "at most 16 rows" "$FIX/drive/rows" --model "model=$Q8"
step cap machine update "$NAME" --gpu-memory-limit "$CAPPED" -y || fail "limit the GPU to $CAPPED"
route room "don't fit at the call's start" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=32 temperature:=0
step uncap machine update "$NAME" --gpu-memory-limit off -y || say "uncap failed (the pod ends anyway)"
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: the engine served the text call and every fallback route answered through Transformers' engine (above)"
