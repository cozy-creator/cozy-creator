# Overlay replay

The exerciser is the Runtime's own built-in (`cozy_runtime.bench`, `internal/bench/qwen_overlay.py`), which `drive/` calls (xs-174).

Qwen-Image-Edit-2511's DiT, whose every LoRA site carries a `lora.Hook`, in a plain step loop, called
three times in one run: unarmed, unarmed again, then with the Lightning LoRA armed before the loop, as
Qwen-Image's package arms. The armed latents must equal the same call with graphs bypassed
(`graphs.BYPASS`), byte for byte, and differ from the unarmed ones. `variant=inside` arms the LoRA inside
the step instead: its latents must equal those armed before the loop.

Until Runtime 0.43 the loop was a keyed `denoise_loop`, and the cell checked that its kept program never
replayed unarmed for an armed call and that arming inside the step raised. Runtime 0.43 has no loop
programs.

```sh
cozy run "$PWD/release-gate/suites/execution/drive/overlay_replay" seed:=331 --gpus=1 --lane int8 --on MACHINE --json
```
