"""Tiny Ideogram 4: the two DiTs and FLUX.2's VAE as a Diffusers folder, and Qwen3-VL as Qwen ships it, a few channels
wide (36 language layers: Ideogram taps 0..35). The Qwen head prefers JSON's closers, so the upsampler's constrained
decode writes a short schema-valid caption and stops."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKLFlux2, FlowMatchEulerDiscreteScheduler, Ideogram4Transformer2DModel
from transformers import AutoTokenizer, Qwen3VLConfig, Qwen3VLForConditionalGeneration

from cell import NARROW, Cell, Json, module, qwen_tokenizer, save, size

#: Qwen's text width; Ideogram reads 13 of its layers' states side by side.
WIDTH = 64
LAYERS = 36


def _qwen(folder: Path) -> None:
    config = Qwen3VLConfig(
        text_config={"hidden_size": WIDTH, "intermediate_size": 128, "num_attention_heads": 4, "num_key_value_heads": 2,
                     "head_dim": 16, "num_hidden_layers": LAYERS, "vocab_size": 151936, "max_position_embeddings": 4096,
                     "rope_theta": 5000000, "rope_scaling": {"mrope_interleaved": True, "mrope_section": [4, 2, 2],
                                                            "rope_type": "default"}, "tie_word_embeddings": False},
        vision_config={"depth": 1, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2, "out_hidden_size": WIDTH,
                       "deepstack_visual_indexes": [0], "patch_size": 16, "spatial_merge_size": 2,
                       "temporal_patch_size": 2},
        image_token_id=151655, video_token_id=151656, tie_word_embeddings=False,
    )
    model = module(Qwen3VLForConditionalGeneration(config))
    qwen_tokenizer(folder)
    _closers(model, folder)
    save(model.to(torch.bfloat16), folder)


def _closers(model: torch.nn.Module, folder: Path) -> None:
    """Residual channel 0 is 1 for every token and no layer writes it; the final norm keeps only it; the head reads it
    for the tokens made of `"`, `}` and `]` and for the end of turn. So wherever the caption schema allows a closer the
    model writes one, and a random tiny LM's caption ends in a few dozen tokens."""
    tokenizer = AutoTokenizer.from_pretrained(folder)
    vocab: dict[str, int] = tokenizer.get_vocab()
    closers = [i for token, i in vocab.items() if token and set(token) <= {'"', "}", "]"}]
    closers.append(vocab["<|im_end|>"])
    language = model.get_submodule("model.language_model")
    with torch.no_grad():
        language.get_parameter("embed_tokens.weight")[:, 0] = 1.0
        for index in range(LAYERS):
            language.get_parameter(f"layers.{index}.self_attn.o_proj.weight")[0] = 0.0
            language.get_parameter(f"layers.{index}.mlp.down_proj.weight")[0] = 0.0
        norm = language.get_parameter("norm.weight")
        norm.zero_()
        norm[0] = 1.0
        head = model.get_parameter("lm_head.weight")
        head.zero_()
        head[closers, 0] = 4.0


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    qwen, ideogram = root / "qwen", root / "ideogram"
    _qwen(qwen)
    for name in ("transformer", "unconditional_transformer"):
        save(module(Ideogram4Transformer2DModel(
            in_channels=128, num_layers=2, attention_head_dim=16, num_attention_heads=2, intermediate_size=64,
            adaln_dim=32, llm_features_dim=13 * WIDTH, mrope_section=(4, 2, 2))).to(torch.bfloat16), ideogram / name)
    save(module(AutoencoderKLFlux2(block_out_channels=(8, 8, 16, 16), layers_per_block=1, norm_num_groups=4,
                                   latent_channels=32)).to(torch.bfloat16), ideogram / "vae")
    save(FlowMatchEulerDiscreteScheduler(num_train_timesteps=1000), ideogram / "scheduler")
    (ideogram / "model_index.json").write_text(json.dumps({
        "_class_name": "Ideogram4Pipeline", "_diffusers_version": "0.41.0",
        "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "transformer": ["diffusers", "Ideogram4Transformer2DModel"],
        "unconditional_transformer": ["diffusers", "Ideogram4Transformer2DModel"],
        "vae": ["diffusers", "AutoencoderKLFlux2"]}))
    return {"ideogram-ai/ideogram-4@0.1.1": ideogram, "qwen/qwen3-vl-8b-instruct@0.1.1": qwen}


CAPTION = json.dumps({"high_level_description": "a red fox on a mossy log",
                      "compositional_deconstruction": {"background": "an autumn forest",
                                                       "elements": [{"type": "obj", "desc": "a red fox"}]}})
_SMALL: dict[str, Json] = {"steps": 2, "seed": 7, "megapixels": 0.0625}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "caption", "payload": {"prompt": CAPTION, **_SMALL}, "check": size(256, 256)},
    # The upsampler's constrained decode (lm.generate under the caption schema). Seed 0's draws meet the sampler's
    # infinite noise on a masked token (0.22.1's case 11) on a Runtime before cozy-runtime b061adad6.
    {"entry": "generate", "name": "upsampled", "payload": {"prompt": "a red fox on a mossy log", **_SMALL, "seed": 0}},
    {"entry": "generate", "name": "regions", "payload": {
        "prompt": CAPTION, **_SMALL, "aspect_ratio": "16:9",
        "regions": [{"box": [0.1, 0.1, 0.5, 0.5], "prompt": "a lantern"},
                    {"box": [0.55, 0.6, 0.95, 0.9], "prompt": "hand-lettered", "text": "OPEN"}]}},
    {"entry": "image", "name": "tool", "payload": {"instructions": "a red fox on a mossy log", "seed": 7}},
    {"entry": "quantize", "name": "int8", "payload": {}, "skip": NARROW},
]
