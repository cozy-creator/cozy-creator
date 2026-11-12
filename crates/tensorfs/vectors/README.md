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
| [relayvsr-headers](relayvsr-headers/) | RelayVSR's `wan.safetensors` and `flash.safetensors` (`kopper/RelayVSR@d43dc55`); `relayvsr` keeps their keys in upstream's construction order (`kopperx/RelayVSR@e856773`) as components `wan` and `flash`. |
| [krea2-headers](krea2-headers/) | Krea 2's reference checkpoint (its module names) and four published LoRAs: Krea's Diffusers-spelled one, AI-Toolkit's, and Patil's and tori29umai's control LoRAs (`.A`/`.B` factors, a widened `first` image input kept whole as `img_in.modules_to_save`); `krea2.lora` follows diffusers' Krea 2 LoRA conversion and ComfyUI's spellings. |
| [flux2-headers](flux2-headers/) | FLUX.2 [dev]'s BFL checkpoint and four published LoRAs (fal's Turbo and its ComfyUI export, two AI-Toolkit LoRAs in BFL names); `flux2.lora` maps BFL's fused Q/K/V onto Diffusers' linears, B in thirds. |
| [flux3-klein-headers](flux3-klein-headers/) | FLUX.3 Image Klein's BFL checkpoint and 17 published LoRAs (SimpleTuner's, AI-Toolkit's in BFL names, Wan2GP's and LoRAlab's with per-module alphas, Diffusers', NoiseToNova's PDD Turbo in ComfyUI's and PEFT's exports with per-step heads), the 12 Cozy mirrors among them; `flux3klein.lora`. |
| [fibo2-headers](fibo2-headers/) | Five published FIBO 2 LoRAs, the 4 Cozy mirrors among them; `fibo2.lora` reads the four in Diffusers' names, and leaves the ComfyUI export (block-diagonal fused Q/K/V, FFN padded to 9728) unclaimed. |
| [hunyuan-image-3-lora-headers](hunyuan-image-3-lora-headers/) | Two published HunyuanImage 3.0 PEFT LoRAs, whose unwrapped names are the `hunyuan-image-3` family's own. |
| [qwen-image-2.1-lora-headers](qwen-image-2.1-lora-headers/) | Published Qwen-Image 2.1 LoRAs: LightX2V's Lightning (`lora.down`/`lora.up`), Turbo8 (Diffusers' `save_lora_weights`), PAI's Fun Acc PDD export (bare `lora_down`/`lora_up`, per-step `proj_out` heads) and e-n-v-y's Fix, a DoRA (`dora_scale`); `qwenimage21.lora` maps them and ComfyUI's, kohya's and PEFT's spellings onto the DiT's linears. Weights under the Qwen Research License; these are headers only. |
| [moss-lora-headers](moss-lora-headers/) | Five published MOSS-Image LoRAs: ComfyUI's and AI-Toolkit's upstream names under `diffusion_model.` (ComfyUI's fused `proj.qkv` among them), kohya's `lora_unet_`; `moss.lora` maps them, a fused Q/K/V's B in thirds. |
| [anima-lora-headers](anima-lora-headers/) | Four published Anima LoRAs: sd-scripts' kohya `lora_unet_` native names, ComfyUI's and AI-Toolkit's under `diffusion_model.`, the LLM adapter's too; `anima.lora` renames them onto the family's Diffusers DiT and its `text_conditioner`. |
| [qwen-image-lora-headers](qwen-image-lora-headers/) | A published Qwen-Image-Edit-2511 LoRA in Diffusers' names under `diffusion_model.`; `qwenimage.lora`. |
| [krea2-comfy-headers](krea2-comfy-headers/) | Krea 2 fine-tunes in the reference checkpoint's names, bare or under ComfyUI's `model.diffusion_model.`: RedCraft 3.0 in bf16, fp8, int8_convrot and nvfp4, Real Dream V3 in fp8, int8_convrot and nvfp4 (Civitai); `krea2.native` and `krea2.comfy` read them onto the family's keys. Headers only. |
| [qwen3vl-comfy-headers](qwen3vl-comfy-headers/) | Quiho's abliterated Qwen3-VL-4B text encoder for Krea 2 in ComfyUI's one-file layout (`model.layers`, `model.visual`), its linears plain F8_E4M3 with no scales; `qwen3vl.comfy` reads it onto krea-2's `text_encoder`, each float8 weight as `fp8.tensorwise` at scale 1. Headers only. |
| [qwen-image-2.1-comfy-headers](qwen-image-2.1-comfy-headers/) | Qwen-Image 2.1 transformers in ComfyUI's single-file layout (fused `img_mlp.gate_up`): Comfy-Org's bf16 repack and Noct-Q V4 in `int8_convrot`; `qwenimage21.comfy` splits them into the family's keys. Headers only. |

H3 normalization has two distinct source contracts: native base QKV rows are interleaved per
head; the reviewed trainer adapter B factors contain contiguous Q/K/V thirds. Native SwiGLU
`[gate; value]` halves become Diffusers `[value; gate]`, including adapter B halves; A and alpha
are unchanged. Names and shapes cannot prove which source layout a producer used.

Preserve the source terms with their fixtures: Anima's
[license](anima-civitai-headers/LICENSE.md) is also retained with its
[Diffusers excerpts](anima-hf-headers/LICENSE.md).
