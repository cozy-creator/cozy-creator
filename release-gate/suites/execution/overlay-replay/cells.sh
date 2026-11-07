# A kept denoise_loop program recorded with no LoRA overlay armed never serves a call that arms one
# (xs-138): one keyed site called unarmed twice (the second replays), then armed, before the loop as
# Qwen-Image's package arms. The armed call must not replay, and its latents must equal eager's.
# It needs the int8 DiT and its LoRA resident (about 23 GB), as lora-overlay-refill does.
if [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -ge 30 ] 2>/dev/null; then
  cell overlay-replay "$HERE/suites/execution/overlay-replay/gpu" seed:=$((R + 331)) --gpus=1 --lane int8
  cell overlay-inside "$HERE/suites/execution/overlay-replay/gpu" variant=inside seed:=$((R + 332)) --gpus=1 --lane int8
fi
