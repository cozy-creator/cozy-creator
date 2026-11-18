# ggml-kernels (xs-213): llama.cpp's MMVQ and MMQ inside our Runtime (`_kernels._ggml`, vendored at <llama>) against the same
# kernels in stock llama.cpp, on one rental of $SKU, at Qwen3.5-9B's and Qwen3.8-27B's linears in q8_0, q4_k, q5_k and q6_k
# at 1, 8, 32 and 512 rows (fixtures/ggml-kernels/shapes.py).
#   ours    the Runtime's GEMM bench, lane `ggml` (fixtures/ggml-kernels/drive: `gemm_speed`), and the pod's copy GB/s
#           (`host_calibrate`), by `cozy run --on` the rental;
#   theirs  stock llama.cpp <llama> built on the pod for its card (CUDA, no flash attention), `test-backend-ops perf` over the
#           same cases (fixtures/ggml-kernels/pod.sh), over ssh.
# compare.py: each case's kernel, both times, ours/theirs and the weight's GB/s; it fails a case slower than 1/0.95 of
# llama.cpp's, or an M=1 MMVQ case on a weight larger than L2 under 75% of the copy GB/s.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (`tensorfs`, `runtime`: wheel sources
# as lib.sh `wheel_from` takes; the Runtime a native dev build carrying `_ggml`).
# params: tensorfs, runtime (required); llama (b12466) and llama_sha256; models (default every q35-9b/q38-27b row); seconds
# (3600); the ride-along's decode, make, model and tree (below).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-kernels
LLAMA=$(param llama b12466)
LLAMA_SUM=$(param llama_sha256 d81544fe1f31f2539f3610486eb10e7fe606d88da9d1f85a54c310bc7ac75af7)
MODELS=$(param models "$(python3 "$FIX/shapes.py" models)")
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · $MODELS"
# The ride-along (optional): `decode` lanes (label=org/name@sha256:…) and `make` lanes (comma-separated recipe lanes of the
# packages checkout `tree`'s qwen3, quantized on the pod from `model`), each timed by ggml-decode's `decode` drive (warm, plain
# and with MTP drafting) on this pod after the kernels: one pod tells a lane's kernels from its bytes (q8_0 against q8_0 with
# its LM head kept bf16, beside int8).
# llama.cpp's side built on the runner first (compiling and linking need no GPU): a build that can't link fails here, at $0,
# not on a pod (44297995601). `arch` names the card's target where the SKU doesn't.
ARCH=$(param arch '')
[ -n "$ARCH" ] || case $SKU in
  rtx-5090* | rtx-5080* | rtx-5070* | rtx-5060* | rtx-pro-6000*) ARCH=120a-real ;;
  rtx-4090* | rtx-4080* | rtx-4070* | rtx-4060* | rtx-6000-ada | l4 | l40*) ARCH=89-real ;;
  rtx-3090* | rtx-3080* | rtx-3070* | rtx-3060* | rtx-a4000 | rtx-a5000 | rtx-a6000 | a40) ARCH=86-real ;;
  *) fail "arch: no target known for $SKU" ;;
esac
if ! W=$OUT/llama-check CUDA=$OUT/llama-check/cuda ARCH=$ARCH BUILD_ONLY=1 timeout 3600 bash "$FIX/pod.sh" "$LLAMA" "$LLAMA_SUM" \
  > "$OUT/llama-check.out" 2>&1; then
  tail -c 1500 "$OUT/llama-check/out/build.log" "$OUT/llama-check.out" 2> /dev/null | tee -a "$R"
  fail "llama.cpp $LLAMA doesn't build for $ARCH on the runner"
fi
say "llama.cpp $LLAMA builds for $ARCH on the runner: $(tail -n 1 "$OUT/llama-check/out/pod.log")"
rm -rf "$OUT/llama-check"
DECODE=$(param decode '') MAKE=$(param make '') MODEL=$(param model '') TREE=$(param tree '')
DFIX=$HERE/fixtures/ggml-decode
. "$DFIX/preflight.sh"
drive_check "$FIX/drive" ggml_drive
drive_lock "$FIX/drive"
if [ -n "$DECODE$MAKE" ]; then
  [ -z "$MAKE" ] || { [[ $MODEL == */*@sha256:* ]] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ]; } ||
    fail "make needs model (org/name@sha256:…) and a packages checkout tree with qwen3"
  kernels "$PACKAGES/$TREE/qwen3"
  drive_check "$DFIX/drive" ggml_logits ${TREE:+"qwen3=$PACKAGES/$TREE/qwen3/uv.lock"}
  drive_lock "$DFIX/drive"
  [ -z "$TREE" ] || drive_lock "$PACKAGES/$TREE/qwen3"
fi
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param seconds 3600)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "\(.name) sm\(.compute_capability // "?")"] | join(", "))' "$OUT/show.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
# The pod's side, every 5 minutes into the report: its last step and what it has on disk.
(while sleep 300; do
  say "pod: $(there "tail -n 1 /w/out/pod.log; du -sh /w/hf /w/build /w/*.gguf 2>/dev/null | tr '\n' ' '; uptime" < /dev/null 2>&1 | tr '\n' ' ' | cut -c1-300)"
done) & BG="$BG $!"

# First, every package this session runs is staged on the pod: its lock and the candidates over it are settled in the pod's
# first minutes, not after llama.cpp's build.
pod_install stage-drive "$FIX/drive"
if [ -n "$DECODE$MAKE" ]; then
  pod_install stage-decode "$DFIX/drive"
  [ -z "$MAKE" ] || pod_install stage-qwen3 "$PACKAGES/$TREE/qwen3"
fi

# Theirs, on the pod's CPUs and then its GPU, while ours waits for it (one GPU: the two never time at once).
there "mkdir -p /w/out" < /dev/null || fail "ssh"
there "cat > /w/pod.sh" < "$FIX/pod.sh" && there "cat > /w/shapes.py" < "$FIX/shapes.py" || fail "copy the fixtures to the pod"
there "bash /w/pod.sh $LLAMA $LLAMA_SUM > /w/out/pod.out 2>&1" 5400 < /dev/null; code=$?
there "cat /w/out/pod.log" < /dev/null | tee -a "$R"
[ "$code" = 0 ] || { there "tail -c 3000 /w/out/pod.out; tail -c 3000 /w/out/build.log" < /dev/null | tee -a "$R"; fail "llama.cpp's side: exit $code"; }
there "cat /w/out/llama-perf.txt" < /dev/null > "$OUT/llama-perf.txt"

# Ours.
step calibrate run "$FIX/drive/host_calibrate" --on "$NAME" || fail "host_calibrate"
on_candidates calibrate
n=$(runno calibrate); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) | jq '.result' > "$OUT/calibrate.json"
benches=()
IFS=, read -ra models <<<"$MODELS"
for model in "${models[@]}"; do
  step "bench-$model" run "$FIX/drive/gemm_speed" lanes=ggml "shapes=$model" --on "$NAME" || fail "gemm_speed $model"
  n=$(runno "bench-$model"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) | jq '.result' > "$OUT/bench-$model.json"
  benches+=("$OUT/bench-$model.json")
done

if [ -n "$DECODE$MAKE" ]; then
  read -ra pairs <<<"$DECODE"
  [ -z "$MAKE" ] || step pull-model model install "$MODEL" --on "$NAME" || fail "pull $MODEL"
  for lane in ${MAKE//,/ }; do
    step "make-$lane" model quantize "$MODEL" --lane "$lane" --package "$PACKAGES/$TREE/qwen3" --on "$NAME" || fail "quantize $lane"
    c=$(made "make-$lane"); [ -n "$c" ] || fail "quantize $lane made no checkpoint"
    pairs+=("$lane=${MODEL%@*}@$c"); say "$lane: ${MODEL%@*}@$c"
  done
  for pair in "${pairs[@]}"; do
    label=${pair%%=*} ref=${pair#*=}
    step "fresh-$label" machine update "$NAME" --gpu-memory-limit off -y > /dev/null || fail "restart TensorD before $label"
    step "pull-$label" model install "$ref" --on "$NAME" || fail "pull $label"
    step "speed-$label" run "$DFIX/drive/decode" --model "model=$ref" --on "$NAME" || fail "speed $label"
    on_candidates "speed-$label"
    n=$(runno "speed-$label"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/speed-$label.json" 2> /dev/null
    say "speed $label: $(jq -r '.result.detail // ""' "$OUT/speed-$label.json") · engine: $(grep -o 'tokens after [0-9]*: [^"]*' "$OUT/speed-$label.json" | head -1)"
    # How often a step reads a plain output head: one short sampled call after the timed ones, told and never judged.
    if step "heads-$label" run "$DFIX/drive/head_reads" --model "model=$ref" --on "$NAME"; then
      n=$(runno "heads-$label")
      say "head reads $label: $( (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail // ""')"
    else
      say "head reads $label: no count ($(tail -n 2 "$OUT/heads-$label.txt" | tr '\n' ' ' | cut -c1-200))"
    fi
  done
fi

(cd "$FIX" && python3 compare.py "$OUT/llama-perf.txt" "$OUT/calibrate.json" "${benches[@]}") > "$OUT/summary.md"; code=$?
cat "$OUT/summary.md" >> "$R"; cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$code" = 0 ] || fail "a case missed its bar (summary.md)"
say "PASS: llama.cpp's kernels in our Runtime at stock llama.cpp's speed on $(jq -r '.status.gpus[0].name' "$OUT/show.json")"
