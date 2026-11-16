# Suite `runtime`: every first-party package's default call on this SKU (products.sh), then a restart and SDXL in a
# second executor. It proves every model still installs, loads, runs, serves its kernels and holds its numbers.
for p in "${PRODUCTS[@]}"; do product "$p"; done
# A 16-bit request attends exactly: Klein's image blocks declare 8-bit attention, its bf16 lane must not take it.
# Escaped from #2050 to cozy-creator #2224: TensorD sent every request bits 0, so `--lane bf16` attended at 8 bits.
cell klein-bf16 "$(pkg flux.3-klein generate_turbo)" --lane bf16 prompt="$OBJECT on a marble plinth, studio photo" seed:=$((R + 40))
if [ -f klein-bf16.json ]; then
  a=$(run_get "$OUT" klein-bf16 '[.measurements.observations[]? | select(.name == "attention.selected") | .value] | unique | join(",")')
  expect klein-bf16-exact "$([ -n "$a" ] && ! grep -qE 'kitchen|sage|fp8|fp4|sol|flashinfer' <<<"$a" && echo PASS || echo FAIL)" \
    "flux.3-klein --lane bf16 attention: ${a:-none recorded}"
fi
# A Gemma 4 checkpoint loads and answers through VisionLM. Escaped (cr-301, cozy-runtime #2212): canonical JSON's 30 for
# its float-only `final_logit_softcapping` failed Transformers' strict config, then its audio feature extractor and its
# unset video defaults (runs IVL-22, 53, 58). Gemma 4 E2B is LTX-2.5's prompt enhancer.
cell vlm-gemma4 "$(pkg qwen3.5 ask)" --model model=google/gemma-4-e2b-it@0.2.0 \
  prompt="Describe $OBJECT on a wooden table in one sentence." max_tokens:=64 seed:=$((R + 41))
if [ -f vlm-gemma4.json ]; then
  t=$(run_get "$OUT" vlm-gemma4 '.result.texts[0] // ""')
  expect vlm-gemma4-answers "$([ -n "$t" ] && echo PASS || echo FAIL)" "Gemma 4 E2B through VisionLM: ${t:0:160}"
fi
# Second executor process: a GPU memory limit write restarts the machine, so every executor is new.
if [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-p2,"* || $ONLY == *",sdxl,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y >restart.log 2>&1; log "restarted $M: $(grep -E '^status' restart.log)"
fi
cell sdxl-p2 "$(pkg sdxl generate)" prompt="$OBJECT on a wooden table, product photo" seed:=$((R + 7))
# SDXL cold, then two warm calls back to back, after each of three restarts (cr-227).
source "$HERE/suites/execution/first-warm-capture/cells.sh"
# A loop step no capture records is refused and runs on eagerly (cr-227).
source "$HERE/suites/execution/capture-refusal/cells.sh"
# A LoRA armed in the forward (H3 Turbo's contract) through paging and base/LoRA alternation (run 19471).
source "$HERE/suites/execution/lora-overlay-refill/cells.sh"
# A child call's own LoRA stack per slot, over the request's --lora (cl-687).
source "$HERE/suites/execution/child-lora-stacks/cells.sh"
# A parent gives its child a file it saved, and nothing else opens (cr-251).
source "$HERE/suites/execution/child-saved-file/cells.sh"
# A weights output: SDXL's int8 recipe re-quantizes its newest release's bf16 commit on the machine. Escaped in Runtime 0.33.0 (xs-105):
# every weights output failed, `open()` passing TensorFS 0.16 a `revision` it no longer takes.
if [ -z "$ONLY" ] || [[ $ONLY == *",quantize-sdxl,"* ]]; then
  q=$(pkg sdxl generate); q=${q/\/generate/}
  src=$(model_details stabilityai/sdxl | jq -r '.releases[0].lanes[] | select(.lane == "bf16") | .checkpoint')
  run_submit "$OUT" quantize-sdxl model quantize "stabilityai/sdxl@$src" --lane int8 --package "$q" --on "$M" > /dev/null; run_wait "$OUT" quantize-sdxl
  s=$(run_get "$OUT" quantize-sdxl .status)
  expect quantize-sdxl-int8 "$([ "$s" = completed ] && echo PASS || echo FAIL)" \
    "model quantize stabilityai/sdxl --lane int8 --package $q: run $(cat quantize-sdxl.number 2>/dev/null) $s $(run_get "$OUT" quantize-sdxl '.reason.message' | head -c 300)"
fi
