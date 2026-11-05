# Overlay replay

A keyed `denoise_loop` over Qwen-Image-Edit-2511's DiT, whose every LoRA site carries a `lora.Hook`,
called three times in one run: unarmed (its first call records), unarmed again (it replays the kept
program), then with the Lightning LoRA armed before the loop, as Qwen-Image's package arms. The armed
call must not replay the unarmed program (the armed overlays are part of its identity), and its
latents must equal the same step run eagerly, byte for byte. `variant=inside` arms the LoRA inside
the step instead, which must raise ("arm overlays before denoise_loop"), with graphs and without.

```sh
cozy run "$PWD/release-gate/suites/execution/overlay-replay/gpu" seed:=331 --gpus=1 --lane int8 --on MACHINE --json
```
