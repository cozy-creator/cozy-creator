# ggml-decode (xs-213): Qwen3.5-9B's ggml lanes in our engine as it is today (VisionLM, its captured decode and MTP drafting),
# beside its bf16 and int8 lanes, on one rental of $SKU. Each lane (`lanes`, label=org/name@sha256:… in order, the first the
# reference):
#   decode   `qwen3/ask` from the packages checkout `tree`, greedy, a 512-token story: the run's own decode note (ms/token, tokens
#            a step) and the linears' kernels, as its record has them;
#   caption  `qwen3/caption` of release-gate/ref/seed.jpg, greedy: the words, held against the reference's;
#   logits   every lane but the reference against it on fixed text (fixtures/ggml-decode/drive `logits`): top-1 agreement and
#            KL(reference || lane) per position.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (wheel sources as lib.sh `wheel_from`
# takes; the Runtime a native dev build carrying `_ggml`). It reports; it fails only on a run that fails.
# params: tree (required), lanes (required), tensorfs, runtime (required), seconds (3600).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
TREE=$(param tree '') LANES=$(param lanes '')
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
read -ra lanes <<<"$LANES"
[ "${#lanes[@]}" -ge 2 ] || fail "lanes: at least a reference and one lane"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · ${lanes[*]}"
kernels "$SOURCE/qwen3"
quote "$(param seconds 3600)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | .name] | join(", "))' "$OUT/show.json")"

record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
STORY='Write a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep writing until you are stopped.'
for pair in "${lanes[@]}"; do
  label=${pair%%=*} ref=${pair#*=}
  step "pull-$label" model install "$ref" --on "$NAME" || fail "pull $label"
  step "decode-$label" run "$SOURCE/qwen3/ask" --model "model=$ref" "prompt=$STORY" max_tokens:=512 temperature:=0 --on "$NAME" ||
    fail "decode $label"
  record "decode-$label"
  say "decode $label: $(grep -o '[0-9]* tokens after [0-9]*: [^"]*' "$OUT/decode-$label.json" | head -1)"
  say "decode $label kernels: $(jq -c '[.. | objects | select(has("gemm")) | .gemm] | unique' "$OUT/decode-$label.json" 2> /dev/null | cut -c1-400)"
  step "caption-$label" run "$SOURCE/qwen3/caption" --model "model=$ref" --file "images=$REF/seed.jpg" temperature:=0 max_tokens:=96 \
    --on "$NAME" || fail "caption $label"
  record "caption-$label"
  jq -r '.result.texts[0] // ""' "$OUT/caption-$label.json" > "$OUT/caption-$label.txt"
  say "caption $label: $(head -c 400 "$OUT/caption-$label.txt")"
done
first=${lanes[0]} ; reflabel=${first%%=*} refref=${first#*=}
for pair in "${lanes[@]:1}"; do
  label=${pair%%=*} ref=${pair#*=}
  same=$(python3 -c "import sys; a, b = (open(p).read().split() for p in sys.argv[1:]); n = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b))); print(f'{n} of {len(a)} words alike')" \
    "$OUT/caption-$reflabel.txt" "$OUT/caption-$label.txt")
  say "caption $label against $reflabel: $same"
  step "logits-$label" run "$FIX/drive/logits" --model "reference=$refref" --model "model=$ref" --on "$NAME" || fail "logits $label"
  record "logits-$label"
  say "logits $label against $reflabel: $(jq -r '.result.detail // ""' "$OUT/logits-$label.json")"
done
say "PASS: every lane decoded, captioned and compared (results above)"
