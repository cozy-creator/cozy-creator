# A LoRA armed in its DiT's own forward stays applied through paging and base/LoRA alternation on one
# executor. Escaped in Runtime 0.33.2: H3 Turbo ran base weights on blocks a scope hold filled before the
# forward armed the overlay (runs 19464, 19480, 19471), and base and Turbo forwards shared pages.
# Sourced by gate.sh (the runtime suite, or --cells); int8 so the LoRA bakes into pages on every SKU.
cell lora-overlay-refill "$HERE/suites/execution/lora-overlay-refill/gpu" \
  seed:=$((R + 321)) --gpus=1 --lane int8 --attention-kernel sdpa
