"""Tiny Anima: the package's own release configs (`anima/configs`) shrunk to a few channels: Cosmos DiT, Qwen3 text
encoder, Anima's text conditioner and the Qwen-Image VAE, as the release's modular Diffusers folder in bfloat16."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import torch
from diffusers import AnimaTextConditioner, AutoencoderKLQwenImage, CosmosTransformer3DModel
from transformers import Qwen3Config, Qwen3Model

from cell import Cell, Json, module, packages, quantize, save, size

WIDTH = 64


def _config(name: str) -> dict[str, Json]:
    loaded: dict[str, Json] = json.loads((packages() / "anima" / "configs" / f"{name}.json").read_text())
    return {k: v for k, v in loaded.items() if not k.startswith("_")}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    out = root / "anima"
    text = {**_config("text_encoder"), "hidden_size": WIDTH, "intermediate_size": 128, "num_hidden_layers": 2,
            "num_attention_heads": 4, "num_key_value_heads": 2, "head_dim": 16, "max_window_layers": 2,
            "layer_types": ["full_attention"] * 2}
    save(module(Qwen3Model(Qwen3Config.from_dict(text))).to(torch.bfloat16), out / "text_encoder")
    save(module(AnimaTextConditioner(**{**_config("text_conditioner"), "source_dim": WIDTH, "target_dim": WIDTH,
                                        "model_dim": WIDTH, "num_layers": 1, "num_attention_heads": 4})
                ).to(torch.bfloat16), out / "text_conditioner")
    save(module(CosmosTransformer3DModel(**{**_config("transformer"), "attention_head_dim": 16, "num_attention_heads": 2,
                                            "num_layers": 2, "adaln_lora_dim": 32, "crossattn_proj_in_channels": WIDTH,
                                            "encoder_hidden_states_channels": WIDTH, "text_embed_dim": WIDTH})
                ).to(torch.bfloat16), out / "transformer")
    save(module(AutoencoderKLQwenImage(**{**_config("vae"), "base_dim": 16, "num_res_blocks": 1})).to(torch.bfloat16),
         out / "vae")
    (out / "scheduler").mkdir()
    shutil.copyfile(packages() / "anima" / "configs" / "scheduler.json", out / "scheduler" / "scheduler_config.json")
    shutil.copyfile(packages() / "anima" / "configs" / "modular_model_index.json", out / "modular_model_index.json")
    return {"circlestone-labs/anima@1.0.0": out}


_SMALL: dict[str, Json] = {"prompt": "masterpiece, a red fox in the snow", "steps": 2, "seed": 7, "megapixels": 1}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": _SMALL, "check": size(1024, 1024)},
    {"entry": "generate", "name": "wide", "payload": {**_SMALL, "aspect_ratio": "16:9", "negative_prompt": "blurry"}},
    {"entry": "generate", "name": "step-cache", "payload": {**_SMALL, "steps": 4, "first_block_cache": 0.5}},
    {"entry": "image", "name": "tool", "payload": {"instructions": "a red fox in the snow", "megapixels": "1", "seed": 7}},
    quantize("circlestone-labs/anima@1.0.0", "fp8", "anima", "anima@0.6.1"),
]
