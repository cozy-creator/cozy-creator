# ggml-decode (xs-213): Qwen3.5-9B's ggml lanes in our engine as it is today (VisionLM, its captured decode and MTP drafting),
# beside its bf16 and int8 lanes and stock llama.cpp, on one rental of $SKU. Each lane (`lanes`, label=org/name@sha256:… in
# order, the first the reference), read by its commit as the session that wrote it named it (no lane choice, no Hub table):
#   speed    the `decode` drive (fixtures/ggml-decode/drive): 512 greedy tokens of one prompt, never stopping, with the
#            checkpoint's MTP head drafting and without, wall tokens/s, warm;
#   decode   `qwen3/ask` from the packages checkout `tree`, greedy, a 512-token story: the run's own decode note (ms/token,
#            tokens a step) and the linears' kernels, as its record has them;
#   caption  `qwen3/caption` of release-gate/ref/seed.jpg, greedy: the words, held against the reference's;
#   logits   every lane but the reference against it on fixed text (the `logits` drive): top-1 agreement and
#            KL(reference || lane) per position.
# Then `capped` (label=limit): that lane's `qwen3/ask` with the GPU memory limited, so its weights stream and its token
# table gathers its rows on the host (cr-332), 64 tokens. Last, with TensorD restarted and the GPU free, stock llama.cpp
# (`llama`, its CUDA release build, unpatched, a measuring harness only) decodes the drive's prompt from `gguf`
# ("<hf repo> <file>") plain and with MTP (fixtures/ggml-decode/llama.sh).
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (wheel sources as lib.sh `wheel_from`
# takes; the Runtime a native dev build carrying `_ggml`). A lane that fails is reported and the rest still run; the session
# fails at its end if any did.
# params: tree (required), lanes (required), tensorfs, runtime (required), capped (q8_0=6GiB; "" for none),
#         llama (b12466; "" for none), gguf (unsloth/Qwen3.5-9B-MTP-GGUF Qwen3.5-9B-Q8_0.gguf), seconds (3600),
#         only ("engine": each lane through the drive's `engine` (the engine's decoder against today's path on the same
#         call), then stock llama.cpp; no comparison of lanes, no capped run; xs-213 phase 2),
#         cases ("": varena drive cases (suites/varena/drive `core`, as kv-tiers.sh runs them) first, on the same pod;
#         engine-step is the engine's step on a small random Qwen3.5 against Transformers, several rows a bucket).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
TREE=$(param tree '') LANES=$(param lanes '') CAPPED=$(param capped q8_0=6GiB) LLAMA=$(param llama b12466) ONLY=$(param only '')
GGUF=$(param gguf 'unsloth/Qwen3.5-9B-MTP-GGUF Qwen3.5-9B-Q8_0.gguf') CASES=$(param cases '')
VARENA=$(cd "$HERE/../suites/varena/drive" && pwd)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
read -ra lanes <<<"$LANES"
[ "${#lanes[@]}" -ge 2 ] || [ "$ONLY" = engine ] || fail "lanes: at least a reference and one lane"
[[ -z $CAPPED || ${CAPPED#*=} =~ ^[0-9]+[MG]iB$ ]] || fail "capped: label=<n>GiB"
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version) · ${lanes[*]}"
kernels "$SOURCE/qwen3"
# The runs' environments, checked on the runner before anything is rented (fixtures/ggml-decode/preflight.sh).
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits "qwen3=$SOURCE/qwen3/uv.lock"
drive_lock "$FIX/drive"
[ -z "$CASES" ] || { drive_check "$VARENA" varena_drive; drive_lock "$VARENA"; }
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param seconds 3600)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | .name] | join(", "))' "$OUT/show.json")"
there() { timeout "${2:-600}" cozy machine ssh "$NAME" -- "$1"; }
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"

record() { local n; n=$(runno "$1"); [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> /dev/null; }
STORY='Write a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep writing until you are stopped.'
FAILED=()
# fresh <step>: TensorD drained and restarted (a settings-only update), so no executor of another package holds a model on
# the GPU while this one measures (each package runs in its own executor: 44290207312's qwen3 runs fell to Transformers'
# loop beside the drive's resident lane).
fresh() { step "fresh-$1" machine update "$NAME" --gpu-memory-limit off -y > /dev/null || fail "restart TensorD before $1"; }
# ran <step>: the step's run used this machine's candidate Runtime and TensorFS, not its lock's pair (TensorD says so when
# it falls back); else the session ends here, before it spends the pod on runs that measure the wrong wheels.
ran() {
  ! grep -qE "it runs on (that series|that|its lock's)|wheel carries no kernels" "$OUT/$1.txt" ||
    fail "$1 did not run on the candidate wheels: $(grep -oE "warning: [^.]*(it runs on [^;]*|wheel carries no kernels[^)]*\))" "$OUT/$1.txt" | head -1)"
}
lane() {
  local label=$1 ref=$2
  step "pull-$label" model install "$ref" --on "$NAME" || return 1
  if [ "$ONLY" = engine ]; then
    fresh "engine-$label"
    step "engine-$label" run "$FIX/drive/engine" --model "model=$ref" --on "$NAME" || { ran "engine-$label"; return 1; }
    ran "engine-$label"
    record "engine-$label"
    say "engine $label: $(jq -r '.result.detail // ""' "$OUT/engine-$label.json")"
    return 0
  fi
  fresh "speed-$label"
  step "speed-$label" run "$FIX/drive/decode" --model "model=$ref" --on "$NAME" || { ran "speed-$label"; return 1; }
  ran "speed-$label"
  record "speed-$label"
  say "speed $label: $(jq -r '.result.detail // ""' "$OUT/speed-$label.json")"
  fresh "decode-$label"
  step "decode-$label" run "$SOURCE/qwen3/ask" --model "model=$ref" "prompt=$STORY" max_tokens:=512 temperature:=0 --on "$NAME" ||
    { ran "decode-$label"; return 1; }
  ran "decode-$label"
  record "decode-$label"
  say "decode $label: $(grep -o '[0-9]* tokens after [0-9]*: [^"]*' "$OUT/decode-$label.json" | head -1)"
  say "decode $label kernels: $(jq -c '[.. | objects | select(has("gemm")) | .gemm] | unique' "$OUT/decode-$label.json" 2> /dev/null | cut -c1-400)"
  step "caption-$label" run "$SOURCE/qwen3/caption" --model "model=$ref" --file "images=$REF/seed.jpg" temperature:=0 max_tokens:=96 \
    --on "$NAME" || return 1
  record "caption-$label"
  jq -r '.result.texts[0] // ""' "$OUT/caption-$label.json" > "$OUT/caption-$label.txt"
  say "caption $label: $(head -c 400 "$OUT/caption-$label.txt")"
}
if [ -n "$CASES" ]; then
  pod_install stage-varena "$VARENA"
  for c in $CASES; do
    step "$c" run "$VARENA/core" "case=$c" --on "$NAME" || { tail -c 3000 "$OUT/$c.txt" | tee -a "$R"; FAILED+=("$c"); continue; }
    on_candidates "$c"
    n=$(runno "$c"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail' > "$OUT/$c-case.json"
    say "$c: $(jq -c '.rows[0] | del(.case)' "$OUT/$c-case.json" 2> /dev/null | cut -c1-3000)"
  done
fi
first=${lanes[0]}; reflabel=${first%%=*} refref=${first#*=}
lane "$reflabel" "$refref" || fail "the reference $reflabel"
for pair in "${lanes[@]:1}"; do
  label=${pair%%=*} ref=${pair#*=}
  lane "$label" "$ref" || { say "FAILED: lane $label (its step's output above)"; FAILED+=("$label"); continue; }
  [ "$ONLY" = engine ] && continue
  same=$(python3 -c "import sys; a, b = (open(p).read().split() for p in sys.argv[1:]); n = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b))); print(f'{n} of {len(a)} words alike')" \
    "$OUT/caption-$reflabel.txt" "$OUT/caption-$label.txt")
  say "caption $label against $reflabel: $same"
  fresh "logits-$label"
  step "logits-$label" run "$FIX/drive/logits" --model "reference=$refref" --model "model=$ref" --on "$NAME" ||
    { say "FAILED: logits $label"; FAILED+=("logits-$label"); continue; }
  record "logits-$label"
  say "logits $label against $reflabel: $(jq -r '.result.detail // ""' "$OUT/logits-$label.json")"
done

if [ -n "$CAPPED" ] && [ "$ONLY" != engine ]; then
  label=${CAPPED%%=*} limit=${CAPPED#*=} ref=
  for pair in "${lanes[@]}"; do [ "${pair%%=*}" = "$label" ] && ref=${pair#*=}; done
  [ -n "$ref" ] || fail "capped: no lane $label"
  step "cap" machine update "$NAME" --gpu-memory-limit "$limit" -y || fail "limit the GPU memory to $limit"
  if step "capped-$label" run "$SOURCE/qwen3/ask" --model "model=$ref" "prompt=$STORY" max_tokens:=64 temperature:=0 --on "$NAME"; then
    record "capped-$label"
    say "capped $label at $limit: $(grep -o '[0-9]* tokens after [0-9]*: [^"]*' "$OUT/capped-$label.json" | head -1); token table: $(jq -r '"filled \([.. | objects | select(has("trace")) | .trace[]? | select(tostring | test("embed_tokens"))] | length) times, gathered on the host \([.. | objects | select(has("gathered")) | .gathered | numbers] | add // "?") times"' "$OUT/capped-$label.json" 2> /dev/null)"
  else
    say "FAILED: $label at $limit (its step's output above)"; FAILED+=("capped-$label")
  fi
fi
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
say "PASS: every lane decoded, captioned and compared, beside stock llama.cpp (results above)"
