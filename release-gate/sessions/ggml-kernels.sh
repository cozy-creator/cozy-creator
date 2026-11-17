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
# (3600).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-kernels
LLAMA=$(param llama b12466)
LLAMA_SUM=$(param llama_sha256 d81544fe1f31f2539f3610486eb10e7fe606d88da9d1f85a54c310bc7ac75af7)
MODELS=$(param models "$(python3 "$FIX/shapes.py" models)")
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · $MODELS"
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

# Theirs, on the pod's CPUs and then its GPU, while ours waits for it (one GPU: the two never time at once).
there "mkdir -p /w/out" < /dev/null || fail "ssh"
there "cat > /w/pod.sh" < "$FIX/pod.sh" && there "cat > /w/shapes.py" < "$FIX/shapes.py" || fail "copy the fixtures to the pod"
there "bash /w/pod.sh $LLAMA $LLAMA_SUM > /w/out/pod.out 2>&1" 5400 < /dev/null; code=$?
there "cat /w/out/pod.log" < /dev/null | tee -a "$R"
[ "$code" = 0 ] || { there "tail -c 3000 /w/out/pod.out; tail -c 3000 /w/out/build.log" < /dev/null | tee -a "$R"; fail "llama.cpp's side: exit $code"; }
there "cat /w/out/llama-perf.txt" < /dev/null > "$OUT/llama-perf.txt"

# Ours.
step calibrate run "$FIX/drive/host_calibrate" --on "$NAME" || fail "host_calibrate"
n=$(runno calibrate); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) | jq '.result' > "$OUT/calibrate.json"
benches=()
IFS=, read -ra models <<<"$MODELS"
for model in "${models[@]}"; do
  step "bench-$model" run "$FIX/drive/gemm_speed" lanes=ggml "shapes=$model" --on "$NAME" || fail "gemm_speed $model"
  n=$(runno "bench-$model"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) | jq '.result' > "$OUT/bench-$model.json"
  benches+=("$OUT/bench-$model.json")
done

(cd "$FIX" && python3 compare.py "$OUT/llama-perf.txt" "$OUT/calibrate.json" "${benches[@]}") > "$OUT/summary.md"; code=$?
cat "$OUT/summary.md" >> "$R"; cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$code" = 0 ] || fail "a case missed its bar (summary.md)"
say "PASS: llama.cpp's kernels in our Runtime at stock llama.cpp's speed on $(jq -r '.status.gpus[0].name' "$OUT/show.json")"
