# engine-routes (xs-213 phase 2): with CozyEngine switched on (a Runtime whose `vlm.COZY_ENGINE` is True), every route a
# `VisionLM.ask` call takes, through `cozy run` on one fresh rental of $SKU, each read from the run's own `.engine` note.
# No run may compile one of the engine's Triton kernels on its request (a `triton compile` confession of one): they are
# its model's declared set, built while the model loads (`engine.kernels`). Any other kernel a request compiles is told.
#   cold       the pod's first q8_0 call: the engine, or Transformers' engine while its set builds ("are building");
#              then the same short call until the engine serves it (`ready`), the time that took told; int8 likewise;
#   served     the q8_0 lane, text (`qwen3/ask`): the engine serves it ("by the engine");
#   int8       the int8.convrot lane, text, 512 tokens: the engine; then `int8-warm`, the same call again, its timing told;
#   q8-warm    q8_0, 512 tokens, no penalty, twice: the second's timing, beside stock llama.cpp's on the same pod (`llama`);
#   picture    the q8_0 lane with a picture: Transformers' engine ("reads text only");
#   conversation  a text question after an earlier turn with a picture (the decode drive's `routes`): the same;
#   grammar    a call held to a grammar (`allowed=lm.Grammar(...)`, the drive's `routes`): Transformers' engine, and its
#              answer is digits only though the question asks for a word, so the constraint held;
#   grammar-cost  four rows sharing a grammar of nearly the whole vocabulary: its cost a token a row, `lm.Grammar`'s and
#              the old one's (the drive's `routes`);
#   ngram      a call with `no_repeat_ngram` (the drive's `routes`): Transformers' engine;
#   ceiling    512 tokens asked of an engine whose rows hold 256: the engine, clamped at the start with a warning;
#   bucket     the real 9B's four rows in a bucket through the model's declared set, drafted and plain, against
#              Transformers' teacher-forced forward at every clear position (the decode drive's `bucket`);
#   unknown    the q4_k_m lane (ggml.q4_k and q6_k, outside the engine's LANES): Transformers' engine;
#   rows       17 questions in one `VisionLM.ask` (the drive's `rows`; AskRequest takes 16): Transformers' engine;
#   room       the q8_0 lane with the GPU memory limited to `capped`: its weights' lease is refused at the call's start, so
#              Transformers' engine serves it ("don't fit at the call's start").
# Then the varena suite's `cases` (engine-loop, a section a process), the refusal path (`refused`: the launcher check's
# records cleared and the next executor's check child killed with SIGSEGV, as a crash ends it: its calls take the
# replayed path or Transformers' engine with the reason, with the same tokens, none crashed, no launcher pass kept), and,
# with TensorD restarted and the GPU free, stock llama.cpp (`llama`).
# Every call must answer with words. The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime.
# `only` (default all): the routes to run, by name (`served refused`: what a launcher-check change touches); the
# pulls, `cold` and the records follow what they serve.
# params: tree (required), tensorfs, runtime (required), only, capped (6GiB), cases (engine-loop), llama (b12466; "" for none),
#         gguf (unsloth/Qwen3.5-9B-MTP-GGUF Qwen3.5-9B-Q8_0.gguf), seconds (2400), skus and quoted (preflight.sh).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/ggml-decode
VARENA=$(cd "$HERE/../suites/varena/drive" && pwd)
TREE=$(param tree '') CAPPED=$(param capped 6GiB) LLAMA=$(param llama b12466) ONLY=$(param only '')
want() { [ -z "$ONLY" ] || [[ " $ONLY " == *" $1 "* ]]; }
CASES=$(param cases "engine-loop-loop engine-loop-polled engine-loop-attention engine-loop-ggml engine-loop-copies engine-loop-replayed")
GGUF=$(param gguf 'unsloth/Qwen3.5-9B-MTP-GGUF Qwen3.5-9B-Q8_0.gguf')
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
[[ $CAPPED =~ ^[0-9]+[MG]iB$ ]] || fail "capped: <n>GiB"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
Q8=qwen/qwen3.5-9b@sha256:c1059b7b191259ecd579df4640b6abd4e419aa25761135f0bd12c8e4b040393c
INT8=qwen/qwen3.5-9b@sha256:37a131b745e22e0c074592f349d6fd6778e5b127dd2a4b277e64ffd053742bd7
Q4=qwen/qwen3.5-9b@sha256:60f445fa7b568f666e6c44b7e53532ddaa486016529946b05d8ab00b8c10dde9
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
kernels "$SOURCE/qwen3"
. "$FIX/preflight.sh"
drive_check "$FIX/drive" ggml_logits "qwen3=$SOURCE/qwen3/uv.lock"
drive_lock "$FIX/drive"
[ -z "$CASES" ] || { drive_check "$VARENA" varena_drive; drive_lock "$VARENA"; }
if [ -n "$LLAMA" ] || want refused || want records; then  # each reads or writes the pod over `cozy machine ssh`
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
pick_offer
quote "$(param seconds 2400)"
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
# compiled <step>: how many of the engine's kernels the run compiled on its request; others <step>: the rest, named.
ENGINE_KERNELS='(rms_norm|add_norm|silu_mul|qk_norm_rope|keep|gated_norm|write_step|paged_step|paged_join|conv_span|delta_span|slots)_kernel'
compiled() { jq --arg k "$ENGINE_KERNELS" '[.. | strings | select(test("^triton compile: " + $k + "$"))] | length' "$OUT/$1.json" 2> /dev/null || echo "?"; }
others() { jq -r --arg k "$ENGINE_KERNELS" '[.. | strings | select(test("^triton compile:") and (test("^triton compile: " + $k + "$") | not))] | unique | join(", ")' "$OUT/$1.json" 2> /dev/null; }
# jitted <step>: the engine's launches that went through Triton's JIT, and why (its run warnings).
jitted() { jq -r '[.. | strings | select(test("went through Triton.s JIT"))] | unique | join("; ")' "$OUT/$1.json" 2> /dev/null; }
# route <step> <the note's words> <run args...>: the call answers with words, its note says the route, and if the
# engine served it, it compiled nothing.
route() {
  want "$1" || return 0
  local n=$1 want=$2 said heard made; shift 2
  step "$n" run "$@" --on "$NAME" || { tail -c 2000 "$OUT/$n.txt" | tee -a "$R"; FAILED+=("$n"); return; }
  record "$n"
  heard=$(note "$n") made=$(compiled "$n")
  said=$(jq -r '.result.texts[0] // .result.detail // ""' "$OUT/$n.json" 2> /dev/null)
  say "$n: engine note: ${heard:-none} · the engine's kernels compiled on the request: $made$(o=$(others "$n"); [ -n "$o" ] && echo " · others: $o")$(j=$(jitted "$n"); [ -n "$j" ] && echo " · $j") · answer: $(head -c 300 <<<"$said")"
  [[ $heard == *"$want"* ]] || { say "FAILED: $n: its note doesn't say '$want'"; FAILED+=("$n"); return; }
  [ "$made" = 0 ] || { say "FAILED: $n: an engine kernel compiled on the request"; FAILED+=("$n"); return; }
  [ -n "${said//[[:space:]]/}" ] || { say "FAILED: $n: no words"; FAILED+=("$n"); }
}
# ready <label> <model>: a short call until the engine serves it (its set landed), at most 20 tries 15 s apart; each
# compiles nothing on its request whichever engine serves it.
ready() {
  local label=$1 model=$2 i began heard
  began=$(date +%s)
  for i in $(seq 1 20); do
    step "$label-$i" run "$SOURCE/qwen3/ask" --model "model=$model" "prompt=Name a colour." max_tokens:=8 temperature:=0 \
      --on "$NAME" || { FAILED+=("$label-$i"); return; }
    record "$label-$i"
    heard=$(note "$label-$i")
    say "$label-$i: the engine's kernels compiled on the request: $(compiled "$label-$i")$(o=$(others "$label-$i"); [ -n "$o" ] && echo " · others: $o")$(j=$(jitted "$label-$i"); [ -n "$j" ] && echo " · $j") · ${heard:-none}"
    [ "$(compiled "$label-$i")" = 0 ] || { say "FAILED: $label-$i: an engine kernel compiled on the request"; FAILED+=("$label-$i"); }
    if [[ $heard == *"by the engine"* ]]; then say "$label: the engine serves from call $i, $(($(date +%s) - began)) s after the first"; return; fi
    sleep 15
  done
  say "FAILED: $label: the engine never served in 20 calls"; FAILED+=("$label")
}
STORY='Write a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep writing until you are stopped.'
FAILED=()
step pull-q8 model install "$Q8" --on "$NAME" || fail "pull q8_0"
want int8 && { step pull-int8 model install "$INT8" --on "$NAME" || fail "pull int8"; }
want unknown && { step pull-q4 model install "$Q4" --on "$NAME" || fail "pull q4_k_m"; }
ready cold "$Q8"
# records <label>: the launcher check's records on the machine (passes by path; crash markers, this boot's).
records() { there "find /var/lib/cozy -name 'cozy-engine-launcher*' -printf '%f\n' 2> /dev/null | sort | paste -sd ' ' -" < /dev/null; }
LAUNCHED=""
for f in "$OUT"/cold-*.json; do [[ $(note "$(basename "$f" .json)") == *", launched)"* ]] && LAUNCHED=1; done
if want records; then
  kept=$(records)
  say "the launcher check's records: ${kept:-none} · the engine $([ -n "$LAUNCHED" ] && echo launched || echo host-replayed)"
  if [ -n "$LAUNCHED" ]; then
    [[ $kept == *"cozy-engine-launcher-passed-"* ]] || { say "FAILED: the launcher served and kept no pass"; FAILED+=("pass-kept"); }
  else
    [[ $kept == *"cozy-engine-launcher-replayed-passed-"* && $kept != *"cozy-engine-launcher-passed-"* ]] ||
      { say "FAILED: host-replayed without the replayed path's pass, or with the launcher's"; FAILED+=("pass-kept"); }
  fi
fi
route served "by the engine" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=64 temperature:=0
want int8 && ready cold-int8 "$INT8"
route int8 "by the engine" "$SOURCE/qwen3/ask" --model "model=$INT8" "prompt=$STORY" max_tokens:=512 temperature:=0
route int8-warm "by the engine" "$SOURCE/qwen3/ask" --model "model=$INT8" "prompt=$STORY" max_tokens:=512 temperature:=0
route q8 "by the engine" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=512 temperature:=0 presence_penalty:=0
route q8-warm "by the engine" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=512 temperature:=0 presence_penalty:=0
route picture "reads text only" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=What is in this picture?" \
  --file "images=$REF/seed.jpg" max_tokens:=32 temperature:=0
route conversation "reads text only" "$FIX/drive/routes" --model "model=$Q8" case=conversation
route grammar "no grammar-constrained decode" "$FIX/drive/routes" --model "model=$Q8" case=grammar
route grammar-cost "no grammar-constrained decode" "$FIX/drive/routes" --model "model=$Q8" case=grammar-cost
route ngram "no repeat-n-gram cut" "$FIX/drive/routes" --model "model=$Q8" case=ngram
route ceiling "by the engine" "$FIX/drive/routes" --model "model=$Q8" case=ceiling
if want ceiling; then
  clamp=$(tr '\r' '\n' < "$OUT/ceiling.txt" | grep -a -m1 -o 'warning: max_tokens [^=]*clamped to [0-9]*')  # the run's warnings, as the CLI says them
  say "ceiling: warning: ${clamp:-none}"
  [ -n "$clamp" ] || { say "FAILED: ceiling: no clamp warning in the run"; FAILED+=("ceiling-warning"); }
fi
if want bucket; then
step bucket run "$FIX/drive/bucket" --model "model=$Q8" --on "$NAME" || { tail -c 2000 "$OUT/bucket.txt" | tee -a "$R"; FAILED+=("bucket"); }
record bucket
jq -r '.result.detail // ""' "$OUT/bucket.json" > "$OUT/bucket-detail.json" 2> /dev/null
say "bucket: $(jq -c '{declared, drafted: [.drafted[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]], plain: [.plain[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]]}' "$OUT/bucket-detail.json" 2> /dev/null)"
jq -e '.declared and ([(.drafted // [])[], (.plain // [])[]] | length == 8) and ([(.drafted // [])[], (.plain // [])[]] | all(.bucket.agreed == .bucket.clear and .alone.agreed == .alone.clear))' \
  "$OUT/bucket-detail.json" > /dev/null 2>&1 || { say "FAILED: bucket: not through the declared set, or a clear position disagreed"; FAILED+=("bucket-forced"); }
# The host-replayed path (`launcher=False`), which a driver whose launcher check refused it serves on: the same check.
step bucket-replayed run "$FIX/drive/bucket_replayed" --model "model=$Q8" --on "$NAME" || { tail -c 2000 "$OUT/bucket-replayed.txt" | tee -a "$R"; FAILED+=("bucket-replayed"); }
record bucket-replayed
jq -r '.result.detail // ""' "$OUT/bucket-replayed.json" > "$OUT/bucket-replayed-detail.json" 2> /dev/null
say "bucket-replayed: $(jq -c '{declared, launcher, drafted: [.drafted[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]], plain: [.plain[]? | [.bucket.agreed, .bucket.clear, .alone.agreed, .alone.clear]]}' "$OUT/bucket-replayed-detail.json" 2> /dev/null)"
jq -e '.declared and (.launcher | not) and ([(.drafted // [])[], (.plain // [])[]] | length == 8) and ([(.drafted // [])[], (.plain // [])[]] | all(.bucket.agreed == .bucket.clear and .alone.agreed == .alone.clear))' \
  "$OUT/bucket-replayed-detail.json" > /dev/null 2>&1 || { say "FAILED: bucket-replayed: a clear position disagreed"; FAILED+=("bucket-replayed-forced"); }
fi
route unknown "which the engine's cells have not passed" "$SOURCE/qwen3/ask" --model "model=$Q4" "prompt=$STORY" \
  max_tokens:=32 temperature:=0
route rows "at most 16 rows" "$FIX/drive/rows" --model "model=$Q8"
if want room; then
  step cap machine update "$NAME" --gpu-memory-limit "$CAPPED" -y || fail "limit the GPU to $CAPPED"
  route room "don't fit at the call's start" "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=32 temperature:=0
  step uncap machine update "$NAME" --gpu-memory-limit off -y || say "uncap failed"
fi
if [ -n "$CASES" ]; then
  pod_install stage-varena "$VARENA"
  for c in $CASES; do
    if step "$c" run "$VARENA/core" "case=$c" --on "$NAME"; then
      on_candidates "$c"
      n=$(runno "$c"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail' > "$OUT/$c-case.json"
      say "$c: $(jq -c '.rows[0] | del(.case)' "$OUT/$c-case.json" 2> /dev/null | cut -c1-1500)"
      continue
    fi
    code=$(grep -ao '"exit": -\?[0-9]*' "$OUT/$c.txt" | tail -1 | grep -o -- '-\?[0-9]*$')
    say "$c: failed, exit ${code:-?}"
    # A launcher section's crash is this driver's, which the launcher check refused (the engine host-replayed).
    if [[ -z $LAUNCHED && ${code:-0} -lt 0 && $c =~ ^engine-loop-(loop|polled|copies)$ ]]; then continue; fi
    tail -c 2000 "$OUT/$c.txt" | tee -a "$R"; FAILED+=("$c")
  done
fi
# The refusal path on any driver: the launcher check's records cleared, TensorD restarted, and the next executor's
# check has its child killed with SIGSEGV a second after it starts (fixtures/ggml-decode/kill-check.sh), as a crash
# ends it. That executor's calls go to the replayed path or Transformers' engine with the reason, none crashed, and no
# launcher pass is kept. The replayed path's check must still pass, in a child that outlives the call it began in
# (engine-routes 44633999562: such a child lost its working directory, and both paths were refused).
if want refused; then
  there "find /var/lib/cozy \\( -name 'cozy-engine-launcher*' -o -name 'fault-cozy-engine-launcher*' \\) -delete" < /dev/null
  step restart-check machine update "$NAME" --gpu-memory-limit off -y || fail "restart TensorD"
  { there "mkdir -p /w" < /dev/null && there "cat > /w/kill-check.sh" < "$FIX/kill-check.sh" &&
    there "setsid nohup bash /w/kill-check.sh > /w/kill-check.log 2>&1 < /dev/null &" < /dev/null; } || fail "start the check's killer"
  refused=""
  for i in $(seq 1 20); do
    step "refused-$i" run "$SOURCE/qwen3/ask" --model "model=$Q8" "prompt=$STORY" max_tokens:=64 temperature:=0 --on "$NAME" \
      || { say "FAILED: refused-$i: a call crashed or failed"; FAILED+=("refused-$i"); break; }
    record "refused-$i"
    heard=$(note "refused-$i")
    said=$(jq -r '.result.texts[0] // ""' "$OUT/refused-$i.json")
    say "refused-$i: ${heard:-none} · answer: $(head -c 120 <<<"$said")"
    if [[ $heard == *"host-replayed: the launcher"* ]]; then refused=$i; break; fi
    [[ $heard != *"neither of the engine's launch paths"* ]] || { say "FAILED: refused-$i: the replayed path's check failed too"; FAILED+=("refused-replayed"); break; }
    [[ $heard == *"being checked"* || $heard == *"by the engine"* ]] || { say "FAILED: refused-$i: an unexpected route"; FAILED+=("refused-$i"); break; }
    sleep 5
  done
  say "kill-check: $(there 'cat /w/kill-check.log' < /dev/null | tail -2 | paste -sd ';' -)"
  [ -n "$refused" ] || { say "FAILED: the killed check never refused the launcher"; FAILED+=("refused"); }
  if [ -n "$refused" ]; then  # the same tokens as the first 64 of the same call before
    same=$(python3 -c "import json,sys; a,b=(json.load(open(p))['result']['texts'][0] for p in sys.argv[1:]); print(a==b)" "$OUT/served.json" "$OUT/refused-$refused.json" 2> /dev/null)
    say "refused-$refused's answer equals served's (the same call before): ${same:-?}"
    [ "$same" = True ] || { say "FAILED: the replayed path's answer differs"; FAILED+=("refused-tokens"); }
  fi
  kept=$(records)
  say "after the killed check, the launcher check's records: ${kept:-none}"
  [[ $kept != *"cozy-engine-launcher-passed-"* ]] || { say "FAILED: a killed check kept the launcher's pass"; FAILED+=("refused-kept"); }
fi
# The executors' own lines on kernels, the engine and lm (a bounded tail of TensorD's log): each set's shapes and build.
cozy machine logs "$NAME" tensord -n 4000 2> /dev/null | grep -aE '\[kernels\]|\[engine\]|\[lm\]|cozy-engine' > "$OUT/kernel-lines.txt" || true
say "kernel lines: $(wc -l < "$OUT/kernel-lines.txt"); the engine's: $(grep -a 'cozy-engine' "$OUT/kernel-lines.txt" | cut -c1-600 | sort -u | paste -sd ';' -)"
if [ -n "$LLAMA" ] && want llama; then
  step restart machine update "$NAME" --gpu-memory-limit off -y || fail "restart TensorD"  # its executors let the GPU go
  read -r hf file <<<"$GGUF"
  { there "mkdir -p /w/out" < /dev/null && there "cat > /w/llama.sh" < "$FIX/llama.sh"; } || fail "copy llama.sh to the pod"
  if there "bash /w/llama.sh $LLAMA $hf $file" 3600 < /dev/null > "$OUT/llama.jsonl" 2> "$OUT/llama.err"; then
    while read -r row; do say "llama.cpp: $row"; done < "$OUT/llama.jsonl"
  else
    tail -c 2000 "$OUT/llama.err" | tee -a "$R"; FAILED+=("llama.cpp")
  fi
fi
[ "${#FAILED[@]}" = 0 ] || fail "failed: ${FAILED[*]}"
say "PASS: no request compiled; the engine served q8_0 and int8 once their sets landed; every other route went to Transformers' engine (above)"
