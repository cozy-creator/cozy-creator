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
# (3600); built (a pod-session run that built llama.cpp on a runner: below); skus and quoted (preflight.sh pick_offer); the
# ride-along's decode, make, model and tree.
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
# packages checkout `tree`'s qwen3, each quantized on the pod from `model` or, written `lane=org/name@sha256:…`, from that
# bf16 commit), each timed by ggml-decode's `decode` drive (warm, plain and with MTP drafting) on this pod after the kernels:
# one pod tells a lane's kernels from its bytes (q8_0 against q8_0 with its LM head kept bf16, beside int8).
# llama.cpp's side built on the runner first (compiling and linking need no GPU): a build that can't link fails here, at $0,
# not on a pod (44297995601). `arch` names the card's target where the SKU doesn't.
arch_of() {
  case $1 in
    rtx-5090* | rtx-5080* | rtx-5070* | rtx-5060* | rtx-pro-6000*) echo 120a-real ;;
    rtx-4090* | rtx-4080* | rtx-4070* | rtx-4060* | rtx-6000-ada | l4 | l40*) echo 89-real ;;
    rtx-3090* | rtx-3080* | rtx-3070* | rtx-3060* | rtx-a4000 | rtx-a5000 | rtx-a6000 | a40) echo 86-real ;;
    *) return 1 ;;
  esac
}
ARCH=$(param arch '')
if [ -z "$ARCH" ]; then
  IFS=, read -ra cards <<<"$(param skus "$SKU")"
  for s in "${cards[@]}"; do
    a=$(arch_of "$s") || fail "arch: no target known for $s"
    [ -z "$ARCH" ] || [ "$a" = "$ARCH" ] || fail "skus: $s is $a, not $ARCH (one target a session)"
    ARCH=$a
  done
fi
# `built`: a pod-session run whose report says a runner built this llama.cpp for this target (this tree's $0 pre-flight):
# a rented dispatch naming it builds nothing again, so its quote comes minutes after the dispatch, not a build later
# (44392328972 quoted 13 min in, at a price that had moved from the pre-flight's).
BUILT=$(param built '')
if [ -n "$BUILT" ]; then
  gh run download "$BUILT" -R cozy-creator/cozy-creator -n "session-ggml-kernels-$BUILT" -D "$OUT/built" > /dev/null 2>&1
  line=$(grep -m1 "llama.cpp $LLAMA builds for $ARCH on the runner" "$OUT/built/results.txt" 2> /dev/null) ||
    fail "built: pod-session $BUILT's report has no llama.cpp $LLAMA build for $ARCH on a runner"
  say "llama.cpp $LLAMA for $ARCH: built on a runner by pod-session $BUILT (${line#* }); not built again"
  rm -rf "$OUT/built"
elif ! W=$OUT/llama-check CUDA=$OUT/llama-check/cuda ARCH=$ARCH BUILD_ONLY=1 timeout 3600 bash "$FIX/pod.sh" "$LLAMA" "$LLAMA_SUM" \
  > "$OUT/llama-check.out" 2>&1; then
  tail -c 1500 "$OUT/llama-check/out/build.log" "$OUT/llama-check.out" 2> /dev/null | tee -a "$R"
  fail "llama.cpp $LLAMA doesn't build for $ARCH on the runner"
else
  say "llama.cpp $LLAMA builds for $ARCH on the runner: $(tail -n 1 "$OUT/llama-check/out/pod.log")"
  rm -rf "$OUT/llama-check"
fi
DECODE=$(param decode '') MAKE=$(param make '') MODEL=$(param model '') TREE=$(param tree '')
DFIX=$HERE/fixtures/ggml-decode
. "$DFIX/preflight.sh"
drive_check "$FIX/drive" ggml_drive
drive_lock "$FIX/drive"
if [ -n "$DECODE$MAKE" ]; then
  for entry in ${MAKE//,/ }; do
    from=$MODEL; [[ $entry != *=* ]] || from=${entry#*=}
    [[ $from == */*@sha256:* ]] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] ||
      fail "make $entry needs a model (org/name@sha256:…, its own or \`model\`) and a packages checkout tree with qwen3"
  done
  kernels "$PACKAGES/$TREE/qwen3"
  drive_check "$DFIX/drive" ggml_logits ${TREE:+"qwen3=$PACKAGES/$TREE/qwen3/uv.lock"}
  drive_lock "$DFIX/drive"
  [ -z "$TREE" ] || drive_lock "$PACKAGES/$TREE/qwen3"
fi
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
pick_offer
quote "$(param seconds 3600)"
offer_rent
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
# Detached on the pod and polled, each poll its own short ssh: a dropped connection costs one poll, never the step
# (44407508649, 44408751569: their one long ssh dropped in a Hub outage and each waited out its 5400 s).
there "rm -f /w/out/pod.rc; setsid nohup bash -c 'bash /w/pod.sh $LLAMA $LLAMA_SUM > /w/out/pod.out 2>&1; echo \$? > /w/out/pod.rc' \
  > /dev/null 2>&1 < /dev/null &" 60 < /dev/null || fail "start llama.cpp's side on the pod"
until=$(($(date +%s) + 5400)) code=''
while [ -z "$code" ] && [ "$(date +%s)" -lt "$until" ]; do
  sleep 30; code=$(there "cat /w/out/pod.rc 2> /dev/null" 60 < /dev/null) || code=''
done
[ -n "$code" ] || code="no exit after 5400 s"
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
  # A failed bench is told and the session goes on: the ride-along's lanes never wait on the kernel table (44416217064).
  if ! step "bench-$model" run "$FIX/drive/gemm_speed" lanes=ggml "shapes=$model" --on "$NAME"; then
    say "FAILED: gemm_speed $model: $(grep -m1 -E '^Run .* failed|^error' "$OUT/bench-$model.txt" | cut -c1-300)"; continue
  fi
  n=$(runno "bench-$model"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) | jq '.result' > "$OUT/bench-$model.json"
  benches+=("$OUT/bench-$model.json")
done

# quality <label> <ref>: the `quality` drive on one lane (or a bf16 reference) into quality-<label>.json; a lane's
# reference is noted in quality.refs.
quality() {
  local n
  if ! step "quality-$1" run "$DFIX/drive/quality" --model "model=$2" --on "$NAME"; then
    say "quality $1: none ($(tail -n 2 "$OUT/quality-$1.txt" | tr '\n' ' ' | cut -c1-200))"; return 0
  fi
  n=$(runno "quality-$1")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail // ""' > "$OUT/quality-$1.json"
  [[ $1 == bf16-* ]] || echo "${2%@*}@ $1 bf16-${2%@*}" | sed 's|bf16-[^ ]*/|bf16-|' >> "$OUT/quality.refs"
}
if [ -n "$DECODE$MAKE" ]; then
  read -ra pairs <<<"$DECODE"
  # Each lane's bf16 reference for `quality`: its model's bf16 commit (`refs`, space-separated org/name@sha256:…; `model`;
  # a make entry's own).
  declare -A pulled=() BF=()
  for r in $(param refs '') ${MODEL:+"$MODEL"}; do BF[${r%@*}]=$r; done
  for entry in ${MAKE//,/ }; do
    lane=${entry%%=*} from=$MODEL; [[ $entry != *=* ]] || from=${entry#*=}
    BF[${from%@*}]=$from
    [ -n "${pulled[$from]:-}" ] || { step "pull-from-$lane" model install "$from" --on "$NAME" || fail "pull $from"; pulled[$from]=1; }
    # A lane that can't be made is told and the others go on (44420479065: w4a8's upload met the Hub unreachable).
    if ! step "make-$lane" model quantize "$from" --lane "$lane" --package "$PACKAGES/$TREE/qwen3" --on "$NAME"; then
      say "FAILED: quantize $lane: $(grep -m1 '^error' "$OUT/make-$lane.txt" | cut -c1-300)"; continue
    fi
    c=$(made "make-$lane"); [ -n "$c" ] || { say "FAILED: quantize $lane made no checkpoint"; continue; }
    pairs+=("$lane=${from%@*}@$c"); say "$lane: ${from%@*}@$c"
  done
  for pair in "${pairs[@]}"; do
    label=${pair%%=*} ref=${pair#*=}
    step "fresh-$label" machine update "$NAME" --gpu-memory-limit off -y > /dev/null || fail "restart TensorD before $label"
    step "pull-$label" model install "$ref" --on "$NAME" || { say "FAILED: pull $label"; continue; }
    step "speed-$label" run "$DFIX/drive/decode" --model "model=$ref" --on "$NAME" || { say "FAILED: speed $label"; continue; }
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
    quality "$label" "$ref"
  done
  # The bf16 references last, each alone on the card (paged where it is larger), then every lane's top-1 agreement with
  # its reference and both perplexities.
  for name in "${!BF[@]}"; do
    grep -qs "^${name}@" "$OUT/quality.refs" || continue
    label=bf16-${name##*/}
    step "fresh-$label" machine update "$NAME" --gpu-memory-limit off -y > /dev/null || fail "restart TensorD before $label"
    step "pull-$label" model install "${BF[$name]}" --on "$NAME" || fail "pull ${BF[$name]}"
    quality "$label" "${BF[$name]}"
  done
  python3 "$DFIX/agreement.py" "$OUT" | tee -a "$R" "${GITHUB_STEP_SUMMARY:-/dev/null}"
fi

(cd "$FIX" && python3 compare.py "$OUT/llama-perf.txt" "$OUT/calibrate.json" "${benches[@]}") > "$OUT/summary.md"; code=$?
cat "$OUT/summary.md" >> "$R"; cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$code" = 0 ] || fail "a case missed its bar (summary.md)"
say "PASS: llama.cpp's kernels in our Runtime at stock llama.cpp's speed on $(jq -r '.status.gpus[0].name' "$OUT/show.json")"
