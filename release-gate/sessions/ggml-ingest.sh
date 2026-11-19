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
#   also      space-separated label:TYPE=org/name@sha256:… lanes already written, compared too (below)
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
# Detached on the pod, its exit code in /w/out/pod.rc, and only polled from here, each poll its own short ssh: one long ssh
# held 44392334657's step open after its pod ended, and 44408757934's dropped and took llama.cpp's side with it.
there "rm -f /w/out/pod.rc; setsid nohup bash -c 'SCALE=$SCALE bash /w/pod.sh $LLAMA $LLAMA_SUM $HFREPO ${types[*]} > /w/out/pod.out 2>&1; echo \$? > /w/out/pod.rc' \
  > /dev/null 2>&1 < /dev/null &" 60 < /dev/null || fail "start llama.cpp's side on the pod"
LLAMA_UNTIL=$(($(date +%s) + 10800 * SCALE))
llama_rc() { there "cat /w/out/pod.rc 2> /dev/null" 60 < /dev/null; }  # its exit code once it ended, else nothing

# 1. Ours. The package staged first: its lock and the candidates over it are settled before the model's download.
pod_install stage "$PACKAGES/$TREE/$PACKAGE"
step pull model install "$MODEL" --on "$NAME" || fail "pull $MODEL"
declare -A REF
BAD=()  # what failed: a lane of ours, a comparison, llama.cpp's side; the session fails at the end, after judging the rest
for pair in $(param have ''); do REF[${pair%%=*}]=${pair#*=}; done
for lane in "${lanes[@]}"; do
  if [ -n "${REF[$lane]:-}" ]; then
    step "pull-$lane" model install "${REF[$lane]}" --on "$NAME" || fail "pull $lane ${REF[$lane]}"
    continue
  fi
  if ! step "$lane" model quantize "$MODEL" --lane "$lane" --package "./$PACKAGE" --on "$NAME"; then
    grep -m1 '^error' "$OUT/$lane.txt" | tee -a "$R"
    say "FAILED: quantize $lane; the other lanes are still judged"; BAD+=("quantize $lane"); continue
  fi
  on_candidates "$lane"
  c=$(made "$lane"); [ -n "$c" ] || { say "FAILED: quantize $lane made no checkpoint"; BAD+=("quantize $lane"); continue; }
  REF[$lane]="${MODEL%@*}@$c"; say "$lane: ${REF[$lane]}"
  n=$(runno "$lane")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$lane-run.json" 2> /dev/null
done

# 3. Each check as soon as its inputs are on the pod, not after everything: the source once llama.cpp's download is in,
# each lane of ours once llama-quantize's file of its type is, each --pure file once written (44392334657 failed one lane
# and judged none).
checks=(source)
for lane in "${lanes[@]}"; do [ -z "${REF[$lane]:-}" ] || checks+=("$lane"); done
for t in "${pure[@]}"; do checks+=("pure:$t"); done
# `also`: space-separated label:TYPE=org/name@sha256:… lanes already written, each pulled and compared with llama-quantize's
# file of TYPE beside the lanes (an earlier write of one, to tell an intermittent fault from a systematic one).
declare -A TYPE=()
for e in $(param also ''); do
  lt=${e%%=*} label=${e%%:*}; TYPE[$label]=${lt#*:} REF[$label]=${e#*=}
  step "pull-$label" model install "${REF[$label]}" --on "$NAME" || fail "pull $label ${REF[$label]}"
  checks+=("$label")
done
ready() {  # ready <check>: the pod.log line that says llama.cpp's side made its input
  case $1 in
    source) there "grep -q '^[0-9:]* download .* took' /w/out/pod.log" 60 < /dev/null ;;
    pure:*) there "grep -q '^[0-9:]* llama-quantize --pure ${1#pure:} took' /w/out/pod.log" 60 < /dev/null ;;
    *) there "grep -q '^[0-9:]* llama-quantize ${TYPE[$1]:-${1^^}} took' /w/out/pod.log" 60 < /dev/null ;;
  esac
}
judge() {  # judge <check>: compare.py on it, its lines into the report
  local arg=$1 code
  case $1 in
    source) ;;
    pure:*) arg="$1=/w/pure-${1#pure:}.gguf" ;;
    *) arg="${TYPE[$1]:+told:}$1=${REF[$1]}:/w/${TYPE[$1]:-${1^^}}.gguf" ;;  # an `also` lane: judged on its decoded values
  esac
  there "PYTHONPATH=/w/src/gguf-py /w/py/bin/python /w/compare.py /w/hf /var/lib/tensorfs $MODEL $arg" 7200 \
    < /dev/null > "$OUT/compare-${1/:/-}.jsonl" 2> "$OUT/compare-${1/:/-}.err"; code=$?
  sed 's/^/    /' "$OUT/compare-${1/:/-}.jsonl" | tee -a "$OUT/compare.jsonl" >> "$R"
  [ "$code" = 0 ] || { tail -c 1500 "$OUT/compare-${1/:/-}.err" | tee -a "$R"; say "FAILED: $1 differs from llama.cpp's"; BAD+=("$1"); }
  [ "$code" != 0 ] || say "$1: ${TYPE[$1]:+decoded equal to llama.cpp's (bytes told above)}${TYPE[$1]:-byte for byte llama.cpp's}"
}
installed=''
while [ "${#checks[@]}" -gt 0 ]; do
  alive=1; [ -z "$(llama_rc)" ] && [ "$(date +%s)" -lt "$LLAMA_UNTIL" ] || alive=0
  left=()
  for c in "${checks[@]}"; do
    if ready "$c"; then
      [ -n "$installed" ] || {
        there "export PATH=\$HOME/.local/bin:\$PATH; uv pip install -q -p /w/py/bin/python /w/${TENSORFS##*/}" < /dev/null ||
          fail "compare.py's TensorFS on the pod"
        installed=1
      }
      judge "$c"
    else
      left+=("$c")
    fi
  done
  checks=("${left[@]}")
  [ "${#checks[@]}" -gt 0 ] || break
  [ "$alive" = 1 ] || { say "FAILED: llama.cpp's side ended before ${checks[*]}"; BAD+=("${checks[@]}"); break; }
  sleep 60
done
until code=$(llama_rc) && [ -n "$code" ] || [ "$(date +%s)" -ge "$LLAMA_UNTIL" ]; do sleep 60; done
[ -n "$code" ] || code="no exit by its deadline"
there "cat /w/out/pod.log" < /dev/null | tee -a "$R"
[ "$code" = 0 ] || { there "tail -c 3000 /w/out/pod.out; tail -c 2000 /w/out/convert.log" < /dev/null | tee -a "$R"; BAD+=("llama.cpp's side: exit $code"); }
[ "${#BAD[@]}" = 0 ] || fail "${BAD[*]}"
say "PASS: ${LANES}${PURE:+ and --pure $PURE} byte for byte llama-quantize's ($LLAMA) on $HFREPO"
