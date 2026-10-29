# Suite `cli` (gate.sh --suite cli): the `run` and `model quantize` flags only a GPU pod means (e2e/coverage.toml's
# `gate` tier), each checked in its record by ./check.py. A failure this file sees itself goes to cli-fails.txt, which
# check.py reports. Sourced in gate.sh's shell: `cell`, `pkg`, `log`, $CLI, $M, $R and the scenario's $SCENE $CHAR
# $OBJECT. Every cozy call is logged (release-gate.yml runs gate.sh with --cli e2e/bin/cozy) for e2e/coverage.py.
#   --gpus 1 · --lane bf16 · --attention-kernel sdpa · --lora (an SDXL LoRA by its Hugging Face ref)
#   machine update --gpu-memory-limit 12GiB, then off · model quantize --lane int8 / fp8, --package, --on
cli_fail() { log "cli: $*"; echo "$*" >> cli-fails.txt; }

cell cli-gpus "$(pkg sdxl generate)" prompt="$OBJECT on a marble plinth, photo" seed:=$((R + 201)) --gpus 1
cell cli-lane16 "$(pkg sdxl generate)" prompt="$SCENE in winter, photo" seed:=$((R + 202)) --lane bf16
cell cli-attn "$(pkg sdxl generate)" prompt="$CHAR reading a map, film still" seed:=$((R + 203)) --attention-kernel sdpa
cell cli-lora "$(pkg sdxl generate)" prompt="$OBJECT, studio light" seed:=$((R + 204)) steps:=8 \
  --lora model:unet=hf://latent-consistency/lcm-lora-sdxl

"$CLI" machine update "$M" --gpu-memory-limit 12GiB -y > cli-cap.log 2>&1 || cli_fail "machine update --gpu-memory-limit 12GiB: $(tail -c 300 cli-cap.log)"
cell cli-cap "$(pkg sdxl generate)" prompt="$SCENE at noon, photo" seed:=$((R + 205))
"$CLI" machine update "$M" --gpu-memory-limit off -y > cli-cap-off.log 2>&1 || cli_fail "machine update --gpu-memory-limit off: $(tail -c 300 cli-cap-off.log)"

# A model's int8 and fp8 lanes by its package's recipes, from a throwaway repository's copy of anima's original
# weights, deleted at once. The copy names anima's bf16 commit: the Hub records it, and nothing moves.
q=fidika/ci-e2e-q-$R
bf16=$("$CLI" --json model info anima 2>/dev/null | python3 -c 'import json,sys; print(next(l["commit"] for l in json.load(sys.stdin)["releases"][0]["lanes"] if l["lane"] == "bf16"))' 2>/dev/null)
"$CLI" --json model upload "anima@$bf16" "$q" --lane bf16 > cli-copy.ndjson 2>&1 || cli_fail "model upload anima@$bf16 $q: $(tail -c 400 cli-copy.ndjson)"
qc=$("$CLI" --json model info "$q" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["commits"][0]["commit"])' 2>/dev/null)
"$CLI" --json model quantize "$q@$qc" --lane int8 --package anima --on "$M" > cli-quantize.ndjson 2>&1 \
  || cli_fail "model quantize --lane int8: $(tail -c 400 cli-quantize.ndjson)"
"$CLI" --json model quantize "$q@$qc" --lane fp8 --package anima --on "$M" > cli-recipe.ndjson 2>&1 \
  || cli_fail "model quantize --lane fp8: $(tail -c 400 cli-recipe.ndjson)"
"$CLI" --json model info "$q" > cli-quantized.json 2>&1 || cli_fail "model info $q: $(tail -c 300 cli-quantized.json)"
grep -q '"lane": *"int8"' cli-quantized.json || cli_fail "model quantize --lane int8: $q has no int8 commit"
grep -q '"lane": *"fp8"' cli-quantized.json || cli_fail "model quantize --lane fp8: $q has no fp8 commit"
"$CLI" --json model delete "$q" -y > /dev/null 2>&1 || cli_fail "model delete $q failed: delete it by hand"
