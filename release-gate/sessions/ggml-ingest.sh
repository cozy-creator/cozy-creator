# ggml-ingest (xs-213): llama.cpp's lanes written in memory by `cozy model quantize`, byte for byte llama-quantize's. One rental:
#   1. the model's bf16 commit pulled onto the pod, then each lane written from it by the packages checkout's recipe (qwen3's
#      q8_0 and q4_k_m: `ggml.*` encodings by TensorFS's ggml-ref/1, no GGUF and no scratch file);
#   2. on the pod over ssh (fixtures/ggml-ingest/pod.sh): stock llama.cpp <llama> (its llama-quantize built for the CPU,
#      unpatched), the Hugging Face checkpoint `hf`, its bf16 GGUF and llama-quantize's Q8_0 and Q4_K_M of it;
#   3. fixtures/ggml-ingest/compare.py: the Hugging Face tensors equal the bf16 commit's, then every tensor llama-quantize
#      quantized equals ours in type and bytes (after the converter's own row moves; ssm_out by ggml-ref/1 of its moved
#      columns). Any difference fails.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (`tensorfs`, `runtime`: wheel
# sources as lib.sh `wheel_from` takes them). Commits stay private.
# packages_refs: one checkout (`tree`). params:
#   tree      the packages_refs checkout (required)       package   its directory with the recipes (qwen3)
#   model     the bf16 commit, org/name@sha256:… (required)
#   hf        the Hugging Face repo it was ingested from (Qwen/Qwen3.5-9B)
#   lanes     comma-separated lanes (q8_0,q4_k_m); the GGUF type each compares with is its name upper-cased
#   llama     llama.cpp's tag (b12466) and llama_sha256 its source archive's digest
#   tensorfs, runtime   wheel sources (required)
#   seconds   what the cap must buy (5400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-ingest
TREE=$(param tree '') PACKAGE=$(param package qwen3) MODEL=$(param model '') HFREPO=$(param hf Qwen/Qwen3.5-9B)
LANES=$(param lanes q8_0,q4_k_m) LLAMA=$(param llama b12466)
LLAMA_SUM=$(param llama_sha256 d81544fe1f31f2539f3610486eb10e7fe606d88da9d1f85a54c310bc7ac75af7)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/$PACKAGE/pyproject.toml" ] || fail "no $PACKAGE in packages_refs checkout '$TREE'"
[[ $MODEL == */*@sha256:* ]] || fail "model must be org/name@sha256:<bf16 commit>"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
cd "$PACKAGES/$TREE" || fail "no checkout"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param seconds 5400)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · \(.status.cpus // "?") CPUs"' "$OUT/show.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"

# 2, started first: llama.cpp's side runs on the pod's CPUs while ours is written.
there "mkdir -p /w/out" < /dev/null || fail "ssh"
there "cat > /w/pod.sh" < "$FIX/pod.sh" && there "cat > /w/compare.py" < "$FIX/compare.py" &&
  there "cat > /w/${TENSORFS##*/}" < "$TENSORFS" || fail "copy the fixtures to the pod"
there "bash /w/pod.sh $LLAMA $LLAMA_SUM $HFREPO > /w/out/pod.out 2>&1" 7200 < /dev/null & LLAMA_PID=$!

# 1. Ours.
step pull model install "$MODEL" --on "$NAME" || fail "pull $MODEL"
declare -A REF
IFS=, read -ra lanes <<<"$LANES"
for lane in "${lanes[@]}"; do
  step "$lane" model quantize "$MODEL" --lane "$lane" --package "./$PACKAGE" --on "$NAME" || fail "quantize $lane"
  c=$(made "$lane"); [ -n "$c" ] || fail "quantize $lane made no checkpoint"
  REF[$lane]="${MODEL%@*}@$c"; say "$lane: ${REF[$lane]}"
  n=$(runno "$lane")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$lane-run.json" 2> /dev/null
done

wait "$LLAMA_PID"; code=$?
there "cat /w/out/pod.log" < /dev/null | tee -a "$R"
[ "$code" = 0 ] || { there "tail -c 3000 /w/out/pod.out; tail -c 2000 /w/out/convert.log" < /dev/null | tee -a "$R"; fail "llama.cpp's side: exit $code"; }

# 3. Compared.
args=()
for lane in "${lanes[@]}"; do args+=("$lane=${REF[$lane]}:/w/${lane^^}.gguf"); done
there "export PATH=\$HOME/.local/bin:\$PATH; uv pip install -q -p /w/py/bin/python /w/${TENSORFS##*/} &&
  PYTHONPATH=/w/src/gguf-py /w/py/bin/python /w/compare.py /w/hf /var/lib/tensorfs $MODEL ${args[*]}" 3600 \
  < /dev/null > "$OUT/compare.jsonl" 2> "$OUT/compare.err"; code=$?
sed 's/^/    /' "$OUT/compare.jsonl" >> "$R"
[ "$code" = 0 ] || { tail -c 2000 "$OUT/compare.err" | tee -a "$R"; fail "our lanes differ from llama-quantize's (compare.jsonl)"; }
say "PASS: ${LANES} byte for byte llama-quantize's ($LLAMA) on $HFREPO"
