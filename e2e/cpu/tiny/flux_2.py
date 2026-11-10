"""Tiny FLUX.2: [dev] (Mistral3 text encoder and upsampler, embedded guidance, an f32 VAE) and a [klein] (Qwen3, no
guidance, a bf16 VAE), each a Diffusers folder of the pipeline's real classes at a few channels, with Qwen's tokenizer
and a byte-level Mistral one. The text encoders keep the depth the conditioning taps (30 and 27 layers) at 32 wide.

`generate` runs [dev]; `generate_turbo` and the `image` tool are bound to the [klein] (`model=`), which runs distilled
(4 steps) or as Base (CFG). fal's Turbo LoRA builds its sites at [dev]'s class-default geometry (`turbo.py` TurboGraph:
`Flux2Transformer2DModel(**(geometry or {}))`, and `loader.construct` passes no geometry): no tiny [dev] can take it."""

from __future__ import annotations

import json
from pathlib import Path

import tensorfs
import torch
from diffusers import AutoencoderKLFlux2, FlowMatchEulerDiscreteScheduler, Flux2Transformer2DModel
from transformers import AutoModelForImageTextToText, Mistral3Config, Qwen3Config, Qwen3ForCausalLM

from cell import Cell, Json, module, qwen_tokenizer, save, size
from tiny import _shared

DEV, KLEIN, TURBO = "black-forest-labs/flux-2-dev@0.1.0", "tiny/flux-2-klein", "fal/flux-2-dev-turbo@0.1.0"
#: 128² px: the smallest canvas a test reads, on the 16 px grid.
MEGAPIXELS = 128 * 128 / (1 << 20)
TEXT_WIDTH = 32
_TURBO_SITES = ("transformer_blocks.0.attn.to_q", "transformer_blocks.0.attn.to_k", "single_transformer_blocks.0.attn.to_out")


def _transformer(guidance: bool) -> torch.nn.Module:
    return module(Flux2Transformer2DModel(
        patch_size=1, in_channels=16, num_layers=1, num_single_layers=1, attention_head_dim=8, num_attention_heads=2,
        joint_attention_dim=3 * TEXT_WIDTH, timestep_guidance_channels=32, axes_dims_rope=(2, 2, 2, 2),
        guidance_embeds=guidance,
    )).to(torch.bfloat16)


def _vae(dtype: torch.dtype) -> torch.nn.Module:
    return module(AutoencoderKLFlux2(
        block_out_channels=(8, 8, 16, 16), layers_per_block=1, latent_channels=4, norm_num_groups=4,
    )).to(dtype)


def _pipeline(out: Path, pipeline: str, encoder: str, tokenizer: str) -> None:
    save(FlowMatchEulerDiscreteScheduler(shift=3.0, use_dynamic_shifting=True), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": pipeline, "_diffusers_version": "0.37.0", "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", encoder], "tokenizer": ["transformers", tokenizer],
        "transformer": ["diffusers", "Flux2Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLFlux2"]}))


def _dev(out: Path) -> None:
    folder = out / "tokenizer"
    ids = _shared.write(folder, _shared.MISTRAL_SPECIALS, {"bos_token": "<s>", "eos_token": "</s>", "unk_token": "<unk>",
                                                           "pad_token": "<pad>"}, _shared.MISTRAL)
    (folder / "processor_config.json").write_text(json.dumps({
        "image_break_token": "[IMG_BREAK]", "image_end_token": "[IMG_END]", "image_token": "[IMG]", "patch_size": 14,
        "processor_class": "PixtralProcessor", "spatial_merge_size": 2}))
    (folder / "preprocessor_config.json").write_text(json.dumps({
        "do_convert_rgb": True, "do_normalize": True, "do_rescale": True, "do_resize": True,
        "image_mean": [0.48145466, 0.4578275, 0.40821073], "image_std": [0.26862954, 0.26130258, 0.27577711],
        "image_processor_type": "PixtralImageProcessorFast", "patch_size": 14, "processor_class": "PixtralProcessor",
        "resample": 3, "rescale_factor": 1 / 255, "size": {"longest_edge": 224}}))
    config = Mistral3Config(
        text_config={"model_type": "mistral", "vocab_size": _shared.vocab(ids), "hidden_size": TEXT_WIDTH,
                     "intermediate_size": 64, "num_hidden_layers": 30, "num_attention_heads": 4, "num_key_value_heads": 2,
                     "head_dim": 8, "max_position_embeddings": 8192, "rope_theta": 1e6, "tie_word_embeddings": False},
        vision_config={"model_type": "pixtral", "hidden_size": 32, "intermediate_size": 64, "num_hidden_layers": 1,
                       "num_attention_heads": 2, "head_dim": 16, "patch_size": 14, "image_size": 1540},
        image_token_index=ids["[IMG]"], spatial_merge_size=2, vision_feature_layer=-1, multimodal_projector_bias=False,
        tie_word_embeddings=False,
    )
    # Mistral 3's conditional-generation class, by its config (its 32-letter name reads as a key to secret scanners).
    encoder = module(AutoModelForImageTextToText.from_config(config))
    save(encoder.to(torch.bfloat16), out / "text_encoder")
    save(_transformer(guidance=True), out / "transformer")
    save(_vae(torch.float32), out / "vae")
    _pipeline(out, "Flux2Pipeline", type(encoder).__name__, "PixtralProcessor")


def _klein(out: Path) -> None:
    qwen_tokenizer(out / "tokenizer")
    config = Qwen3Config(vocab_size=151936, hidden_size=TEXT_WIDTH, intermediate_size=64, num_hidden_layers=27,
                         num_attention_heads=4, num_key_value_heads=2, head_dim=8, max_position_embeddings=8192,
                         tie_word_embeddings=True)
    save(module(Qwen3ForCausalLM(config)).to(torch.bfloat16), out / "text_encoder")
    save(_transformer(guidance=False), out / "transformer")
    save(_vae(torch.bfloat16), out / "vae")
    _pipeline(out, "Flux2KleinPipeline", "Qwen3ForCausalLM", "Qwen2TokenizerFast")


def _turbo(store: Path) -> str:
    """fal's LoRA layout (PEFT factors at `r` over named linears) over the tiny [dev]'s sites."""
    rank = 4
    with torch.device("meta"):
        dev = _transformer(guidance=True)
    writer = tensorfs.Writer(cache_dir=store)
    sites: list[Json] = list(_TURBO_SITES)
    component = writer.component("model", cls=None, config={"r": rank, "lora_alpha": rank, "target_modules": sites})
    generator = torch.Generator().manual_seed(3)
    for site in _TURBO_SITES:
        linear = dev.get_submodule(site)
        assert isinstance(linear, torch.nn.Linear)
        for factor, shape in (("lora_A", (rank, linear.in_features)), ("lora_B", (linear.out_features, rank))):
            data = (torch.randn(shape, generator=generator) * 0.02).to(torch.bfloat16).contiguous()
            component.put(f"{site}.{factor}.weight", memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16",
                          shape=list(shape))
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    _dev(root / "dev")
    torch.manual_seed(1)
    _klein(root / "klein")
    klein = root / "klein"
    return {DEV: root / "dev", TURBO: _turbo(store), "generate_turbo.models.model": klein, "image.models.model": klein}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 160, 128)
_SNOW = _shared.picture("snow", 96, 96)
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": MEGAPIXELS}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox"), "steps": 2, **_SMALL},
     "check": size(128, 128)},
    {"entry": "generate", "name": "reference", "payload": {"prompt": _prompt("the fox from <fox> in snow", "fox"), "steps": 2,
                                                           "seed": 7},
     "files": {"prompt.references.0.file": _FOX}, "check": size(160, 128)},
    {"entry": "generate", "name": "two-references",
     "payload": {"prompt": _prompt("the fox from <fox> on <snow>", "fox", "snow"), "steps": 2, **_SMALL},
     "files": {"prompt.references.0.file": _FOX, "prompt.references.1.file": _SNOW}, "check": size(144, 112)},
    {"entry": "generate", "name": "upsampled", "payload": {"prompt": _prompt("a red fox"), "steps": 2, "prompt_upsampling": True,
                                                           **_SMALL}},
    {"entry": "generate", "name": "upsampled-reference",
     "payload": {"prompt": _prompt("the fox from <fox> in snow", "fox"), "steps": 2, "prompt_upsampling": True, **_SMALL},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "generate_turbo", "name": "klein", "payload": {"prompt": _prompt("a red fox"), **_SMALL}, "check": size(128, 128)},
    {"entry": "generate_turbo", "name": "klein-reference",
     "payload": {"prompt": _prompt("the fox from <fox> in snow", "fox"), **_SMALL}, "files": {"prompt.references.0.file": _FOX}},
    {"entry": "image", "name": "klein-turbo", "payload": {"instructions": _prompt("a red fox"), "seed": 7},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "klein-base", "payload": {"instructions": _prompt("a red fox"), "seed": 7, "turbo": False,
                                                         "prompt_upsampling": False}, "check": size(1024, 1024)},
    {"entry": "generate_turbo", "name": "dev-turbo-lora", "payload": {},
     "skip": "fal's Turbo LoRA builds its sites at [dev]'s class-default geometry (turbo.py TurboGraph); no tiny [dev] takes it"},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": "weights entry: the driver does not grant Invoke.sources yet"},
]
