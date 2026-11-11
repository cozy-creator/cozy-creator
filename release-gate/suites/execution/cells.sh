# Suite `execution`: every proof that reports `passed` and belongs to no other suite, so none goes unrun; analyze.py fails a
# completed run that says passed: false. The native loop and kernel readiness (core-cells.sh), capture, LoRA overlays and
# child calls (also in `runtime`: a cell one suite ran does not run again), TensorFS's producers and decoders, and SDXL's
# cold and warm qualification. varena's mechanisms are suite `varena`'s (core, faults, ledger, public API).
# What needs the Runtime's internals is a Runtime built-in (`cozy_runtime.bench`) that drive/ calls (xs-174).
# Not here: mps-residency, which needs an Apple GPU (e2e/mps).
EXEC_DIR=$HERE/suites/execution TFS_DIR=$HERE/suites/tensorfs
source "$EXEC_DIR/core-cells.sh"
for part in first-warm-capture capture-refusal lora-overlay-refill overlay-replay child-lora-stacks child-saved-file; do
  source "$EXEC_DIR/$part/cells.sh"
done

proof_seed=$((R + 400))
proof() { local name=$1 target=$2; shift 2; proof_seed=$((proof_seed + 1)); cell "$name" "$target" seed:=$proof_seed "$@"; }
proof bf16-normalize "$TFS_DIR/bf16-normalize/run" mode=basic --gpus=1
proof bf16-normalize-cancel "$TFS_DIR/bf16-normalize/run" mode=cancel --gpus=1
proof rowwise-quantize "$TFS_DIR/rowwise-quantize/run"
proof rowwise-quantize-cancel "$TFS_DIR/rowwise-quantize/run" mode=cancel
proof offline-producers "$TFS_DIR/offline-producers/run" mode=basic --gpus=1
proof offline-producers-cancel-fp8 "$TFS_DIR/offline-producers/run" mode=cancel-fp8 --gpus=1
proof offline-producers-cancel-w4a8 "$TFS_DIR/offline-producers/run" mode=cancel-w4a8 --gpus=1

# A controller of its own through the ordinary CLI: its exit is its verdict.
controller() {
  local name=$1 code=0; shift
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* ]] || return 0
  nice -n 19 uv run -q --no-project --with pillow python -I "$@" > "$name.out" 2>&1 || code=$?
  expect "$name" "$([ "$code" = 0 ] && echo PASS || echo FAIL)" "exit $code: $(tail -n 1 "$name.out" | cut -c1-200)"
}
# Each makes a private model, deleted after.
quantize_proof() {
  local name=$1 model=$WHO/ci-$1-$R
  controller "$name" "$TFS_DIR/$name/run.py" --machine "$M" --model "$model" --seed $((R + $2)) --out "$OUT/$name"
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* ]] || return 0
  "$CLI" model delete "$model" -y > "$name-delete.log" 2>&1 || log "$name: $model not deleted: $(tail -c 200 "$name-delete.log")"
}
WHO=; for _ in 1 2 3; do  # a dropped connection is asked again
  WHO=$("$CLI" --json whoami 2>/dev/null | jq -r '.account // empty') && [ -n "$WHO" ] && break
  sleep 5
done
quantize_proof svdq-calibration 431
quantize_proof recipe-quantize 432
controller model-qualification "$EXEC_DIR/model-qualification/run.py" --target "$(pkg sdxl generate)" \
  --input "$EXEC_DIR/model-qualification/sdxl-request.json" --machine "$M" --lane int8 \
  --seeds $((R + 441)) $((R + 442)) $((R + 443)) $((R + 444)) --steps 30 --expect-size 1024x1024 --output "$OUT/model-qualification"
# TensorFS's own encoders and resident decode: programs on the pod against its public API, not packages (xs-174). Last:
# they restart the machine so no executor holds the card.
programs tensorfs "$TFS_DIR/programs" resident_nvfp4.py nvfp4_encode.py convrot_encode.py checkpoint_quantize.py
