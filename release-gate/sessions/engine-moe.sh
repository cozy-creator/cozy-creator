# engine-moe (xs-213): CozyEngine on a mixture of experts (Qwen3.6-35B-A3B: 256 routed experts a layer, top 8, a shared
# expert, an MTP head that is one MoE layer), on one rental of $SKU: the engine's done bar on each lane in one session.
#   ready      the lane's first calls through `qwen3/ask` (Transformers' engine while the model's set builds) until the
#              engine serves one, how long that took; no call compiles one of the engine's kernels on its request;
#   served     512 greedy tokens of the story through `qwen3/ask`, twice: the engine's note (ms a step, tokens a step) and
#              the second call's wall time;
#   engine     the decode drive's `engine` (fixtures/ggml-decode/drive): 512 tokens drafted and plain through the engine's
#              decoder against today's path (`lm.generate`) on the same pod, warm, each held against Transformers'
#              teacher-forced forward at its clear positions (lead >= 0.5): every clear position must agree, and the
#              engine's best mode must be faster than today's path;
#   bucket     the drive's `bucket`: four rows in one bucket through the model's declared set, drafted and plain, every
#              clear position held the same way, and each row alone;
#   room       the first lane under `capped` GPU memory: the engine's lease is refused at the call's start, so
#              Transformers' engine serves it ("don't fit at the call's start"), and it answers.
# Last, with TensorD restarted and the GPU free, stock llama.cpp (`llama`, unpatched) decodes the drive's prompt from
# `gguf` ("<hf repo> <file>"), plain and with MTP (fixtures/ggml-decode/llama.sh).
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (a native dev build: `_ggml`).
# params: tree (required: a packages checkout with qwen3), lanes (required: label=org/name@sha256:<checkpoint>, or
#         label=org/name for that model's checkpoint in lane <label>; the first the one `room` caps), tensorfs, runtime (required), capped (24GiB; "" for none), llama (b12466; "" for none),
#         gguf (unsloth/Qwen3.6-35B-A3B-MTP-GGUF Qwen3.6-35B-A3B-Q8_0.gguf), seconds (5400), skus and quoted (preflight.sh).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
TREE=$(param tree '') LANES=$(param lanes '') CAPPED=$(param capped 24GiB) LLAMA=$(param llama b12466)
GGUF=$(param gguf 'unsloth/Qwen3.6-35B-A3B-MTP-GGUF Qwen3.6-35B-A3B-Q8_0.gguf')
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
[[ -z $CAPPED || $CAPPED =~ ^[0-9]+[MG]iB$ ]] || fail "capped: <n>GiB"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
read -ra lanes <<<"$LANES"
[ "${#lanes[@]}" -ge 1 ] || fail "lanes: label=org/name@sha256:<checkpoint>, one or more"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · ${lanes[*]}"
# Each lane's checkpoint, as the Hub has it, before anything is rented: a lane given as org/name is that model's
# checkpoint in the lane its label names.
for i in "${!lanes[@]}"; do
  label=${lanes[$i]%%=*} ref=${lanes[$i]#*=}
  cozy --json model show "$ref" > "$OUT/show-$label.json" 2> "$OUT/show-$label.err" ||
    fail "lane $label: the Hub has no $ref ($(tail -c 300 "$OUT/show-$label.err"))"
  if [[ $ref != *@* ]]; then
    say "lanes of $ref: $(jq -c '[.lanes[]? | {lane, checkpoint}]' "$OUT/show-$label.json" | cut -c1-1500)"
    checkpoint=$(jq -r --arg l "$label" '[.lanes[]? | select(.lane == $l) | .checkpoint] | first // ""' "$OUT/show-$label.json")
    [ -n "$checkpoint" ] || fail "lane $label: $ref has no $label lane"
    ref=$ref@$checkpoint lanes[$i]=$label=$ref
  fi
  say "lane $label: $ref · $(jq -c '[.lanes[]? | {lane, checkpoint}]' "$OUT/show-$label.json" | cut -c1-400)"
done
kernels "$SOURCE/qwen3"
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits "qwen3=$SOURCE/qwen3/uv.lock"
drive_lock "$FIX/drive"
if [ -n "$LLAMA" ]; then
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
pick_offer
quote "$(param seconds 5400)"
offer_rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "\(.name), driver \(.driver // "?")"] | join(", "))' "$OUT/show.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"

record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
# note <step>: the run's `.engine` note (which engine served it, and why not the engine).
note() { jq -r '[.. | objects | to_entries[] | select((.key | endswith(".engine")) and (.value | type == "string")) | .value] | first // ""' "$OUT/$1.json" 2> /dev/null; }
# compiled <step>: how many of the engine's kernels (its own, the router's and combine's, the grouped experts') the run
# compiled on its request.
ENGINE_KERNELS='(rms_norm|add_norm|silu_mul|qk_norm_rope|keep|gated_norm|write_step|paged_step|paged_join|conv_span|delta_span|slots|route|combine|experts_bf16|experts_int8|grouped_bf16|grouped_int8)_kernel'
compiled() { jq --arg k "$ENGINE_KERNELS" '[.. | strings | select(test("^triton compile: " + $k + "$"))] | length' "$OUT/$1.json" 2> /dev/null || echo "?"; }
jitted() { jq -r '[.. | strings | select(test("went through Triton.s JIT"))] | unique | join("; ")' "$OUT/$1.json" 2> /dev/null; }
memo() { jq -r '[.. | objects | select(has("memoized")) | .memoized] | any' "$OUT/$1.json" 2> /dev/null; }
# route <step> <the note's words> <run args...>: the call answers with words, its note says the route, it compiled none
# of the engine's kernels, and it rendered (no memo).
route() {
  local n=$1 want=$2 said heard made; shift 2
  step "$n" run "$@" --on "$NAME" || { tail -c 2000 "$OUT/$n.txt" | tee -a "$R"; FAILED+=("$n"); return; }
  record "$n"
  heard=$(note "$n") made=$(compiled "$n")
  said=$(jq -r '.result.texts[0] // .result.detail // ""' "$OUT/$n.json" 2> /dev/null)
  say "$n: engine note: ${heard:-none} · the engine's kernels compiled on the request: $made$(j=$(jitted "$n"); [ -n "$j" ] && echo " · $j") · answer: $(head -c 300 <<<"$said")"
  [ "$(memo "$n")" != true ] || { say "FAILED: $n: served from its memo, so it timed nothing"; FAILED+=("$n"); return; }
  [[ $heard == *"$want"* ]] || { say "FAILED: $n: its note doesn't say '$want'"; FAILED+=("$n"); return; }
  [ "$made" = 0 ] || { say "FAILED: $n: an engine kernel compiled on the request"; FAILED+=("$n"); return; }
  [ -n "${said//[[:space:]]/}" ] || { say "FAILED: $n: no words"; FAILED+=("$n"); }
}
# ready <label> <model>: a short call until the engine serves it (its set built, ggml's kernels verified), at most 40
# tries 15 s apart; none compiles one of the engine's kernels on its request.
ready() {
  local label=$1 model=$2 i began heard
  began=$(date +%s)
  for i in $(seq 1 40); do
    step "$label-$i" run "$SOURCE/qwen3/ask" --model "model=$model" "prompt=Name a colour." max_tokens:=8 temperature:=0 \
      --on "$NAME" || { FAILED+=("$label-$i"); return; }
    record "$label-$i"
    heard=$(note "$label-$i")
    say "$label-$i: the engine's kernels compiled on the request: $(compiled "$label-$i")$(j=$(jitted "$label-$i"); [ -n "$j" ] && echo " · $j") · ${heard:-none}"
    [ "$(compiled "$label-$i")" = 0 ] || { say "FAILED: $label-$i: an engine kernel compiled on the request"; FAILED+=("$label-$i"); }
    if [[ $heard == *"by the engine"* ]]; then say "$label: the engine serves from call $i, $(($(date +%s) - began)) s after the first"; return; fi
    sleep 15
  done
  say "FAILED: $label: the engine never served in 40 calls"; FAILED+=("$label")
}
# fresh <step>: TensorD drained and restarted, so the drive's executor holds the only model on the GPU.
fresh() { step "fresh-$1" machine update "$NAME" --gpu-memory-limit off -y > /dev/null || fail "restart TensorD before $1"; }
# drive <entry> <label> <model>: the decode drive's entrypoint on a fresh TensorD; its detail in $OUT/<entry>-<label>-detail.json.
drive() {
  local entry=$1 label=$2 model=$3
  fresh "$entry-$label"
  step "$entry-$label" run "$FIX/drive/$entry" --model "model=$model" --on "$NAME" ||
    { tail -c 2000 "$OUT/$entry-$label.txt" | tee -a "$R"; FAILED+=("$entry-$label"); return 1; }
  record "$entry-$label"
  jq -r '.result.detail // ""' "$OUT/$entry-$label.json" > "$OUT/$entry-$label-detail.json" 2> /dev/null
}
STORY='Write a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep writing until you are stopped.'
FAILED=()
for pair in "${lanes[@]}"; do
  label=${pair%%=*} ref=${pair#*=}
  step "pull-$label" model install "$ref" --on "$NAME" || { FAILED+=("pull-$label"); continue; }
  fresh "ready-$label"
  ready "ready-$label" "$ref"
  route "served-$label" "by the engine" "$SOURCE/qwen3/ask" --model "model=$ref" "prompt=$STORY" max_tokens:=512 temperature:=0
  route "warm-$label" "by the engine" "$SOURCE/qwen3/ask" --model "model=$ref" "prompt=$STORY" max_tokens:=512 temperature:=0
  if drive engine "$label" "$ref"; then
    d=$OUT/engine-$label-detail.json
    say "engine $label: today $(jq -c '.today' "$d") · drafted $(jq -c '.drafted | {s, tok_s, ms_step, kept_a_step, captured, launcher, notes}' "$d") · plain $(jq -c '.plain | {s, tok_s, ms_step, captured, launcher, notes}' "$d") · cold $(jq -c '.engine_cold_s' "$d")"
    say "engine $label forced: $(jq -c '.forced | with_entries(.value |= [.agreed, .clear, .first_wrong])' "$d")"
    say "engine $label profile: $(jq -c '.profile_launched // .profile | {steps, kernels_a_step, device_ms_a_step, top: .top[:12]}' "$d" | cut -c1-2500)"
    jq -e '[.forced | to_entries[] | select(.key != "today") | .value | .agreed == .clear] | all' "$d" > /dev/null ||
      { say "FAILED: engine $label: a clear position disagreed"; FAILED+=("forced-$label"); }
    jq -e '([.drafted.s, .plain.s] | map(select(. != null)) | min) < .today.s' "$d" > /dev/null ||
      { say "FAILED: engine $label: no faster than today's path"; FAILED+=("speed-$label"); }
  fi
  if drive bucket "$label" "$ref"; then
    d=$OUT/bucket-$label-detail.json
    say "bucket $label: $(jq -c '{declared, launcher, drafted: [.drafted[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear, .same_as_alone]], plain: [.plain[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear, .same_as_alone]]}' "$d")"
    jq -e '.declared and ([(.drafted // [])[], (.plain // [])[]] | length >= 4) and ([(.drafted // [])[], (.plain // [])[]] | all(.bucket.agreed == .bucket.clear and .alone.agreed == .alone.clear))' "$d" > /dev/null ||
      { say "FAILED: bucket $label: not through the declared set, or a clear position disagreed"; FAILED+=("bucket-$label"); }
  fi
done
if [ -n "$CAPPED" ]; then
  first=${lanes[0]}
  step cap machine update "$NAME" --gpu-memory-limit "$CAPPED" -y || fail "limit the GPU to $CAPPED"
  route room "don't fit at the call's start" "$SOURCE/qwen3/ask" --model "model=${first#*=}" "prompt=$STORY" max_tokens:=32 temperature:=0
fi
# The executors' own lines on kernels, the engine and lm, with their times (a bounded tail of TensorD's log).
cozy machine logs "$NAME" tensord -n 4000 2> /dev/null | grep -aE '\[kernels\]|\[engine\]|\[lm\]|\[moe\]|Building|opening' > "$OUT/kernel-lines.txt" || true
say "kernel lines: $(wc -l < "$OUT/kernel-lines.txt"); first: $(head -3 "$OUT/kernel-lines.txt" | cut -c1-200 | paste -sd ';' -)"
# A settings-only update drains and restarts TensorD: its executors let the GPU go before llama.cpp starts.
step uncap machine update "$NAME" --gpu-memory-limit off -y || fail "restart TensorD"
if [ -n "$LLAMA" ]; then
  read -r hf file <<<"$GGUF"
  { there "mkdir -p /w/out" < /dev/null && there "cat > /w/llama.sh" < "$FIX/llama.sh"; } || fail "copy llama.sh to the pod"
  if there "bash /w/llama.sh $LLAMA $hf $file" 3600 < /dev/null > "$OUT/llama.jsonl" 2> "$OUT/llama.err"; then
    while read -r row; do say "llama.cpp: $row"; done < "$OUT/llama.jsonl"
  else
    tail -c 2000 "$OUT/llama.err" | tee -a "$R"; FAILED+=("llama.cpp")
  fi
fi
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: every lane served by the engine, teacher-forced at every clear position, faster than today's path; the capped call fell back (results above)"
