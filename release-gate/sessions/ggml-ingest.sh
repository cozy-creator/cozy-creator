# ggml-ingest (xs-213): llama.cpp's lanes written in memory by `cozy model quantize`, byte for byte llama-quantize's. One rental:
#   1. the model's bf16 commit pulled onto the pod, then each lane written from it by the packages checkout's recipe (qwen3's
#      q8_0 and q4_k_m: `ggml.*` encodings by TensorFS's ggml-ref/1, no GGUF and no scratch file);
#   2. on the pod over ssh (fixtures/ggml-ingest/pod.sh): stock llama.cpp <llama> (its llama-quantize built for the CPU,
#      unpatched), the Hugging Face checkpoint `hf`, its bf16 GGUF and llama-quantize's file of each lane's type and of
#      each `pure` type (--pure: every tensor in that type);
#   3. fixtures/ggml-ingest/compare.py: the Hugging Face tensors equal the bf16 commit's, then every tensor llama-quantize
#      quantized equals ours in type and bytes (after the converter's own row moves; ssm_out by ggml-ref/1 of its moved
#      columns), and every tensor of a --pure file equals ggml-ref/1 of the same source. Any difference fails.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (`tensorfs`, `runtime`: wheel
# sources as lib.sh `wheel_from` takes them). Commits stay private.
# packages_refs: one checkout (`tree`). params:
#   tree      the packages_refs checkout (required)       package   its directory with the recipes (qwen3)
#   model     the bf16 commit, org/name@sha256:… (required)
#   hf        the Hugging Face repo it was ingested from (Qwen/Qwen3.5-9B)
#   lanes     comma-separated lanes (q8_0,q4_k_m); the GGUF type each compares with is its name upper-cased
#   pure      comma-separated llama-quantize types (Q5_0,Q5_1,Q5_K_S) made with --pure: every tensor llama-quantize
#             quantizes in that one type, each compared with ggml-ref/1 of the same source, no lane written
#   llama     llama.cpp's tag (b12466) and llama_sha256 its source archive's digest
#   tensorfs, runtime   wheel sources (required)
#   have      space-separated lane=org/name@sha256:… already written: pulled, not quantized again
#   seconds   what the cap must buy (5400)
#   scale     multiplies the pod's phase deadlines (1; 2 for the 35B MoE)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-ingest
TREE=$(param tree '') PACKAGE=$(param package qwen3) MODEL=$(param model '') HFREPO=$(param hf Qwen/Qwen3.5-9B)
LANES=$(param lanes q8_0,q4_k_m) PURE=$(param pure '') LLAMA=$(param llama b12466)
LLAMA_SUM=$(param llama_sha256 d81544fe1f31f2539f3610486eb10e7fe606d88da9d1f85a54c310bc7ac75af7)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/$PACKAGE/pyproject.toml" ] || fail "no $PACKAGE in packages_refs checkout '$TREE'"
[[ $MODEL == */*@sha256:* ]] || fail "model must be org/name@sha256:<bf16 commit>"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
# The package locked as `cozy model quantize --package` will lock it, on the runner, before anything is rented, and the
# candidates checked to stand in for that lock (fixtures/ggml-decode/preflight.sh).
. "$HERE/fixtures/ggml-decode/preflight.sh"
drive_lock "$PACKAGES/$TREE/$PACKAGE"
said=$(uv run -q --no-project --with packaging python "$STANDS" "$PACKAGES/$TREE/$PACKAGE/uv.lock" "$PACKAGE" "$RUNTIME" "$TENSORFS") ||
  fail "$said"
say "$said"
kernels "$PACKAGES/$TREE/$PACKAGE"
cd "$PACKAGES/$TREE" || fail "no checkout"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
pick_offer
quote "$(param seconds 5400)"
offer_rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · \(.status.cpus // "?") CPUs"' "$OUT/show.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
# The pod's side, every 5 minutes into the report: its last step and what it has on disk.
(while sleep 300; do
  say "pod: $(there "tail -n 1 /w/out/pod.log; du -sh /w/hf /w/build /w/*.gguf 2>/dev/null | tr '\n' ' '; uptime" < /dev/null 2>&1 | tr '\n' ' ' | cut -c1-300)"
done) & BG="$BG $!"

# 2, started first: llama.cpp's side runs on the pod's CPUs while ours is written.
there "mkdir -p /w/out" < /dev/null || fail "ssh"
there "cat > /w/pod.sh" < "$FIX/pod.sh" && there "cat > /w/compare.py" < "$FIX/compare.py" &&
  there "cat > /w/${TENSORFS##*/}" < "$TENSORFS" || fail "copy the fixtures to the pod"
IFS=, read -ra lanes <<<"$LANES"
IFS=, read -ra pure <<<"$PURE"
types=("${lanes[@]^^}")
for t in "${pure[@]}"; do types+=("pure:$t"); done
SCALE=$(param scale 1); [[ $SCALE =~ ^[1-9]$ ]] || fail "scale: 1 to 9"
# In BG, so the trap stops it: left running, its ssh held the step's output open for its timeout after the pod had ended
# (44392334657).
there "SCALE=$SCALE bash /w/pod.sh $LLAMA $LLAMA_SUM $HFREPO ${types[*]} > /w/out/pod.out 2>&1" $((10800 * SCALE)) < /dev/null & LLAMA_PID=$!
BG="$BG $LLAMA_PID"

# 1. Ours. The package staged first: its lock and the candidates over it are settled before the model's download.
pod_install stage "$PACKAGES/$TREE/$PACKAGE"
step pull model install "$MODEL" --on "$NAME" || fail "pull $MODEL"
declare -A REF
for pair in $(param have ''); do REF[${pair%%=*}]=${pair#*=}; done
for lane in "${lanes[@]}"; do
  if [ -n "${REF[$lane]:-}" ]; then
    step "pull-$lane" model install "${REF[$lane]}" --on "$NAME" || fail "pull $lane ${REF[$lane]}"
    continue
  fi
  step "$lane" model quantize "$MODEL" --lane "$lane" --package "./$PACKAGE" --on "$NAME" || fail "quantize $lane"
  on_candidates "$lane"
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
for t in "${pure[@]}"; do args+=("pure:$t=/w/pure-$t.gguf"); done
there "export PATH=\$HOME/.local/bin:\$PATH; uv pip install -q -p /w/py/bin/python /w/${TENSORFS##*/} &&
  PYTHONPATH=/w/src/gguf-py /w/py/bin/python /w/compare.py /w/hf /var/lib/tensorfs $MODEL ${args[*]}" 7200 \
  < /dev/null > "$OUT/compare.jsonl" 2> "$OUT/compare.err"; code=$?
sed 's/^/    /' "$OUT/compare.jsonl" >> "$R"
[ "$code" = 0 ] || { tail -c 2000 "$OUT/compare.err" | tee -a "$R"; fail "our lanes differ from llama-quantize's (compare.jsonl)"; }
say "PASS: ${LANES}${PURE:+ and --pure $PURE} byte for byte llama-quantize's ($LLAMA) on $HFREPO"
