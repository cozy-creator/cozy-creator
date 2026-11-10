"""Tiny Krea 2: the Diffusers folder Krea ships (`Krea2Transformer2DModel` with its norms in F32 as
`_keep_in_fp32_modules` stores them, Qwen3-VL as `Qwen3VLModel`, the Qwen-Image VAE in F32) at a few channels, with
Qwen's tokenizer. The text model keeps the 35 layers the DiT taps (2, 5, ..., 35), 32 wide. Raw and Turbo share
the configs and the weights; the function names the regime."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKLQwenImage, FlowMatchEulerDiscreteScheduler, Krea2Transformer2DModel
from transformers import Qwen3VLModel

from cell import NARROW, Cell, module, qwen_tokenizer, save, size
from tiny import _shared

RAW, TURBO = "krea/krea-2@0.1.0", "krea/krea-2-turbo@0.1.0"
TEXT_WIDTH, Z = 32, 16


def _transformer() -> torch.nn.Module:
    transformer = module(Krea2Transformer2DModel(
        in_channels=4 * Z, num_layers=2, attention_head_dim=16, num_attention_heads=2, num_key_value_heads=1,
        intermediate_size=64, timestep_embed_dim=32, text_hidden_dim=TEXT_WIDTH, num_text_layers=12,
        text_num_attention_heads=2, text_num_key_value_heads=2, text_intermediate_size=64, axes_dims_rope=(4, 6, 6),
    )).to(torch.bfloat16)
    kept = {"norm", "norm1", "norm2", "norm_q", "norm_k"}
    for name, child in transformer.named_modules():
        if name.rsplit(".", 1)[-1] in kept:
            child.to(torch.float32)
    return transformer


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "krea-2"
    torch.manual_seed(0)
    qwen_tokenizer(out / "tokenizer")
    save(module(Qwen3VLModel(_shared.qwen3_vl(35, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    save(_transformer(), out / "transformer")
    save(module(AutoencoderKLQwenImage(base_dim=8, z_dim=Z, num_res_blocks=1)), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(use_dynamic_shifting=True, base_shift=0.5, max_shift=1.15, base_image_seq_len=256,
                                         max_image_seq_len=6400, shift_terminal=None), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "Krea2Pipeline", "_diffusers_version": "0.42.1", "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", "Qwen3VLModel"], "tokenizer": ["transformers", "Qwen2TokenizerFast"],
        "transformer": ["diffusers", "Krea2Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLQwenImage"]}))
    return {RAW: out, TURBO: out}


#: 512², Krea's smallest canvas (0.25 MP).
_SMALL = {"prompt": "a red fox in fresh snow", "seed": 7, "megapixels": 0.25, "steps": 2}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "generate", "name": "negative", "payload": {**_SMALL, "negative_prompt": "blurry", "guidance": 5.0}},
    {"entry": "generate", "name": "no-cfg", "payload": {**_SMALL, "guidance": 0.0}},
    {"entry": "generate", "name": "wide", "payload": {**_SMALL, "aspect_ratio": "16:9"}, "check": size(688, 384)},
    {"entry": "generate_turbo", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "image", "name": "turbo", "payload": {"instructions": "a red fox in fresh snow", "seed": 7, "megapixels": 1},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "raw", "payload": {"instructions": "a red fox in fresh snow", "seed": 7, "megapixels": 1,
                                                  "turbo": False}, "check": size(1024, 1024)},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
