# A LoRA overlay armed before a plain step loop, as Qwen-Image's package arms (xs-138): the DiT called
# unarmed twice, then armed; the armed latents must equal the same call with graphs bypassed, and an
# overlay armed inside the step must give the same latents. (Until Runtime 0.43: a kept denoise_loop.)
# It needs the int8 DiT and its LoRA resident (about 23 GB), as lora-overlay-refill does.
if [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -ge 30 ] 2>/dev/null; then
  cell overlay-replay "$HERE/suites/execution/overlay-replay/gpu" seed:=$((R + 331)) --gpus=1 --lane int8
  cell overlay-inside "$HERE/suites/execution/overlay-replay/gpu" variant=inside seed:=$((R + 332)) --gpus=1 --lane int8
fi
