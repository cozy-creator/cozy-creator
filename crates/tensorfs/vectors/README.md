# Source-header fixtures

These fixtures contain safetensors length prefixes and JSON headers, plus shard indexes where
needed. They contain no trained tensor payloads. Each directory's `MANIFEST.json` pins source
revisions, file identities, lengths and measured header hashes. Provider-declared whole-file
hashes are not locally verified payload hashes; sparse carriers prove geometry, not values or
inference quality. Conversion checks must use an independent source-to-output oracle.

| Directory | Coverage |
|---|---|
| [anima-civitai-headers](anima-civitai-headers/) | Anima single-file transformer, text encoder and Qwen-Image VAE; mapping follows the upstream Diffusers converter. |
| [anima-hf-headers](anima-hf-headers/) | The corresponding Diffusers folder and its source key sets. |
| [civitai-sdxl-headers](civitai-sdxl-headers/) | A Civitai SDXL single file; its source body was independently downloaded and hash-verified, but only its header is retained here. |
| [sdxl-headers](sdxl-headers/) | The upstream SDXL folder, including per-member identities and shapes. |
| [h3-latent-upscaler-headers](h3-latent-upscaler-headers/) | LBH-123-AI's H3 3D-conv latent upscaler (`LBH-123-AI/Minimax_h3_latent_Upscaler@3f941d5`, sha256 `4f57821f…`); `h3.latent_upscaler` keeps its keys in `LatentResizer3D`'s construction order. |
| [krea2-headers](krea2-headers/) | Krea 2's reference checkpoint (its module names) and two published LoRAs, Krea's Diffusers-spelled one and AI-Toolkit's; `krea2.lora` follows diffusers' Krea 2 LoRA conversion and ComfyUI's spellings. |
| [flux2-headers](flux2-headers/) | FLUX.2 [dev]'s BFL checkpoint and four published LoRAs (fal's Turbo and its ComfyUI export, two AI-Toolkit LoRAs in BFL names); `flux2.lora` maps BFL's fused Q/K/V onto Diffusers' linears, B in thirds. |
| [flux3-klein-headers](flux3-klein-headers/) | FLUX.3 Image Klein's BFL checkpoint and six published LoRAs (SimpleTuner's, AI-Toolkit's in BFL names, NoiseToNova's PDD Turbo in ComfyUI's and PEFT's exports with per-step heads); `flux3klein.lora`. |
| [fibo2-headers](fibo2-headers/) | Four published FIBO 2 LoRAs; `fibo2.lora` reads the three in Diffusers' names, and leaves the ComfyUI export (block-diagonal fused Q/K/V, FFN padded to 9728) unclaimed. |
| [hunyuan-image-3-lora-headers](hunyuan-image-3-lora-headers/) | Two published HunyuanImage 3.0 PEFT LoRAs, whose unwrapped names are the `hunyuan-image-3` family's own. |
| [qwen-image-2.1-lora-headers](qwen-image-2.1-lora-headers/) | Three published Qwen-Image 2.1 LoRAs: LightX2V's Lightning (`lora.down`/`lora.up`), Turbo8 (Diffusers' `save_lora_weights`) and PAI's Fun Acc PDD export (bare `lora_down`/`lora_up`, per-step `proj_out` heads); `qwenimage21.lora` maps them and ComfyUI's, kohya's and PEFT's spellings onto the DiT's linears. Weights under the Qwen Research License; these are headers only. |

H3 normalization has two distinct source contracts: native base QKV rows are interleaved per
head; the reviewed trainer adapter B factors contain contiguous Q/K/V thirds. Native SwiGLU
`[gate; value]` halves become Diffusers `[value; gate]`, including adapter B halves; A and alpha
are unchanged. Names and shapes cannot prove which source layout a producer used.

Preserve the source terms with their fixtures: Anima's
[license](anima-civitai-headers/LICENSE.md) is also retained with its
[Diffusers excerpts](anima-hf-headers/LICENSE.md).
