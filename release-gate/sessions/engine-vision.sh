# engine-vision (xs-213): CozyEngine serves questions with pictures and videos (cozy-runtime #2357), on one fresh rental of $SKU.
# First, at $0 on this runner: the candidates stand in for qwen3's lock and the drive's (fixtures/ggml-decode/preflight.sh); then,
# with them on this runner's own machine, the drive's `inputs` on the real Qwen3.5-0.8B on its CPU: each case's prompt as
# CozyEngine reads and encodes it, through the language model's own forward (whole and 64 tokens at a time), gives Transformers'
# forward's top token at the prompt's end and its rope delta (the largest logit difference told).
# Then on the pod, real Qwen3.5-9B on the q8_0 and int8.convrot lanes, each lane:
#   cold      a short text call until the engine serves it (its kernel set landed);
#   inputs    the CPU check on the GPU;
#   pictures  the engine's decoder on a picture, a picture in an earlier turn, a short video and text, as one bucket of four rows
#             (four rope deltas) and each alone, drafted and plain, every row against Transformers' teacher-forced forward with
#             the pixel values at every clear position; tokens kept a step on each;
#   caption   Caption's question on both engines on one executor, warm, the same seeds: CozyEngine must be faster a call.
# And on q8_0, through `cozy run` as a person runs them, each note "by the engine", no engine kernel compiled on the request:
# `qwen3/caption` (a mentioned picture), `qwen3/ask` with a picture and with a video file, the drive's `ask` with a picture in an
# earlier turn. The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime.
# params: tree (required), tensorfs, runtime (required), small (the CPU check's model), seconds (2400), skus and quoted.
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
DRIVE=$HERE/fixtures/engine-vision/drive
TREE=$(param tree '') SMALL=$(param small huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
command -v ffmpeg > /dev/null || fail "no ffmpeg"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
Q8=qwen/qwen3.5-9b@sha256:c1059b7b191259ecd579df4640b6abd4e419aa25761135f0bd12c8e4b040393c
INT8=qwen/qwen3.5-9b@sha256:37a131b745e22e0c074592f349d6fd6778e5b127dd2a4b277e64ffd053742bd7
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
mkdir -p "$OUT/inputs"
ffmpeg -v error -y -f lavfi -i testsrc=duration=2:size=320x240:rate=10 -pix_fmt yuv420p "$OUT/inputs/clip.mp4" || fail "no clip"
kernels "$SOURCE/qwen3"
. "$FIX/preflight.sh"
drive_check "$DRIVE" engine_vision "qwen3=$SOURCE/qwen3/uv.lock"
drive_lock "$DRIVE"

record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
detail() { jq -r '.result.detail // ""' "$OUT/$1.json" > "$OUT/$1-detail.json" 2> /dev/null; }
# note <step>: the run's `.engine` note (which engine served it, and why not the engine).
note() { jq -r '[.. | objects | to_entries[] | select((.key | endswith(".engine")) and (.value | type == "string")) | .value] | first // ""' "$OUT/$1.json" 2> /dev/null; }
ENGINE_KERNELS='(rms_norm|add_norm|silu_mul|qk_norm_rope|keep|gated_norm|write_step|paged_step|paged_join|conv_span|delta_span|slots)_kernel'
compiled() { jq --arg k "$ENGINE_KERNELS" '[.. | strings | select(test("^triton compile: " + $k + "$"))] | length' "$OUT/$1.json" 2> /dev/null || echo "?"; }
FAILED=()
# inputs <step> <machine> <model>: the drive's `inputs` there; every media case's top token and rope delta are Transformers'.
inputs() {
  step "$1" run "$DRIVE/inputs" --model "model=$3" --file "image=$REF/seed.jpg" --on "$2" ||
    { tail -c 2000 "$OUT/$1.txt" | tee -a "$R"; FAILED+=("$1"); return; }
  record "$1"; detail "$1"
  say "$1: $(jq -c . "$OUT/$1-detail.json" 2> /dev/null | cut -c1-1500)"
  jq -e '.engine == "CozyEngine" and ([.[] | objects | select(.media_tokens != null)] | length == 3 and
         all(.whole_top_same and .chunked_top_same and .delta == .their_delta))' "$OUT/$1-detail.json" > /dev/null 2>&1 ||
    { say "FAILED: $1: a case's top token or rope delta isn't Transformers'"; FAILED+=("$1"); }
}
# The CPU check, on this runner's own machine with the candidates: what a $0 pre-flight is for.
if step local-candidates machine update local --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y; then
  inputs inputs-cpu local "$SMALL"
else
  say "FAILED: the candidates on this runner's machine: $(tail -c 400 "$OUT/local-candidates.txt")"; FAILED+=("local-candidates")
fi
[ "${#FAILED[@]}" = 0 ] || fail "failed before renting: ${FAILED[*]}"

pick_offer
quote "$(param seconds 2400)"
offer_rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "\(.name), driver \(.driver // "?")"] | join(", "))' "$OUT/show.json")"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"

# route <step> <run args...>: the call answers with words, its note says the engine served it, and it compiled none of its kernels.
route() {
  local n=$1 heard made said; shift
  step "$n" run "$@" --on "$NAME" || { tail -c 2000 "$OUT/$n.txt" | tee -a "$R"; FAILED+=("$n"); return; }
  record "$n"
  heard=$(note "$n") made=$(compiled "$n")
  said=$(jq -r '.result.texts[0] // .result.text // .result.detail // ""' "$OUT/$n.json" 2> /dev/null)
  say "$n: engine note: ${heard:-none} · the engine's kernels compiled on the request: $made · answer: $(head -c 300 <<<"$said")"
  [[ $heard == *"by the engine"* ]] || { say "FAILED: $n: not served by the engine"; FAILED+=("$n"); return; }
  [ "$made" = 0 ] || { say "FAILED: $n: an engine kernel compiled on the request"; FAILED+=("$n"); return; }
  [ -n "${said//[[:space:]]/}" ] || { say "FAILED: $n: no words"; FAILED+=("$n"); }
}
# served_from <label> <model>: a short text call until the engine serves it (its set landed), at most 20 tries 15 s apart.
served_from() {
  local label=$1 model=$2 i began=$SECONDS
  for i in $(seq 1 20); do
    step "$label-$i" run "$SOURCE/qwen3/ask" --model "model=$model" "prompt=Name a colour." max_tokens:=8 temperature:=0 \
      --on "$NAME" || { FAILED+=("$label-$i"); return; }
    record "$label-$i"
    if [[ $(note "$label-$i") == *"by the engine"* ]]; then say "$label: the engine serves from call $i, $((SECONDS - began)) s in"; return; fi
    sleep 15
  done
  say "FAILED: $label: the engine never served in 20 calls"; FAILED+=("$label")
}
for lane in q8:$Q8 int8:$INT8; do
  l=${lane%%:*} model=${lane#*:}
  step "pull-$l" model install "$model" --on "$NAME" || fail "pull $l"
  served_from "cold-$l" "$model"
  inputs "inputs-$l" "$NAME" "$model"
  step "pictures-$l" run "$DRIVE/pictures" --model "model=$model" --file "image=$REF/seed.jpg" --on "$NAME" ||
    { tail -c 2000 "$OUT/pictures-$l.txt" | tee -a "$R"; FAILED+=("pictures-$l"); }
  record "pictures-$l"; detail "pictures-$l"
  say "pictures-$l: $(jq -c '{launcher, tokens, deltas} + ([.drafted, .plain] | map(select(. != null)) | {modes: map(with_entries(.value |= [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear, .kept_a_step]))})' "$OUT/pictures-$l-detail.json" 2> /dev/null)"
  jq -e '.declared and ([.drafted, .plain] | map(select(. != null)) | length == 2 and all(to_entries | length == 4 and
         all(.value.bucket.agreed == .value.bucket.clear and .value.alone.agreed == .value.alone.clear and .value.alone.clear >= 20)))' \
    "$OUT/pictures-$l-detail.json" > /dev/null 2>&1 || { say "FAILED: pictures-$l: a clear position disagreed, or too few"; FAILED+=("pictures-$l-forced"); }
  step "caption-$l" run "$DRIVE/caption" --model "model=$model" --file "image=$REF/seed.jpg" runs:=3 --on "$NAME" ||
    { tail -c 2000 "$OUT/caption-$l.txt" | tee -a "$R"; FAILED+=("caption-$l"); }
  record "caption-$l"; detail "caption-$l"
  say "caption-$l: $(jq -c '{call_x, token_x, engine: [.engine[] | [.s, .tokens, .prefill_s]], transformers: [.transformers[] | [.s, .tokens]]}' "$OUT/caption-$l-detail.json" 2> /dev/null)"
  jq -e '.call_x > 1' "$OUT/caption-$l-detail.json" > /dev/null 2>&1 || { say "FAILED: caption-$l: CozyEngine is no faster a call"; FAILED+=("caption-$l-speed"); }
done
route caption "$SOURCE/qwen3/caption" --model "model=$Q8" "instructions=<picture>" \
  "references:=[{\"name\": \"picture\", \"image\": \"$REF/seed.jpg\"}]" seed:=7
route picture "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=What is in this picture?" --file "images=$REF/seed.jpg" \
  max_tokens:=32 temperature:=0
route video "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=What happens in this video?" --file "videos=$OUT/inputs/clip.mp4" \
  max_tokens:=32 temperature:=0
route earlier "$DRIVE/ask" --model "model=$Q8" --file "image=$REF/seed.jpg" case=earlier
# The executors' own lines on kernels and the engine (a bounded tail of TensorD's log).
cozy machine logs "$NAME" tensord -n 4000 2> /dev/null | grep -aE '\[kernels\]|\[engine\]|\[lm\]' > "$OUT/kernel-lines.txt" || true
say "kernel lines: $(wc -l < "$OUT/kernel-lines.txt")"
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: CozyEngine served every picture, earlier-turn picture and video call; teacher-forced at every clear position; faster than Transformers' engine for Caption"
