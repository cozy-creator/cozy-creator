"""Tiny real PAN2 modules, including Qwen3.5 and Wan2.2, with PAN2's fixed caption-header token ids.

The synthetic word vocabulary is deliberately small. The production crop boundary still sees
[im_start, user, newline] = [248045, 846, 198], rather than waiving the crop check.
Full-size/max-duration quality and latency are qualified separately on H200.
"""
from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKLWan, FlowMatchEulerDiscreteScheduler, PAN2Transformer3DModel
from tokenizers import AddedToken, Regex, Tokenizer, models as token_models, pre_tokenizers
from transformers import Qwen3_5TextConfig, Qwen3_5TextModel

from cell import Cell, Json, module, save
from tiny import _shared, qwen3_5
from tiny._video import video

REF = "ifm/pan2@0.1.0"
_CAPTION = json.dumps({"video_style": "A fixed tabletop shot.", "lighting": "Daylight.",
                       "perspective_view": "Side view.", "subjects_lifecycle": [],
                       "init_frame_visual_description": "A red ball on a table.",
                       "events": [{"event_index": 0, "time_range_sec": "0 to 1", "event_description": "The ball rolls."}],
                       "quality": {"video_quality": "Clear."}}, separators=(",", ":"))


def tokenizer(folder: Path) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    words = {"[UNK]": 0, "\n": 198, "user": 846, "system": 847, "assistant": 848,
             "<|endoftext|>": 248044, "<|im_start|>": 248045, "<|im_end|>": 248046}
    made = Tokenizer(token_models.WordLevel(words, unk_token="[UNK]"))
    made.pre_tokenizer = pre_tokenizers.Sequence([
        pre_tokenizers.Split(Regex(r"[^\S\n]+"), behavior="removed"),
        pre_tokenizers.Split(Regex(r"\n"), behavior="isolated"),
    ])
    specials = ("<|endoftext|>", "<|im_start|>", "<|im_end|>")
    made.add_special_tokens([AddedToken(s, special=True, normalized=False) for s in specials])
    made.save(str(folder / "tokenizer.json"))
    (folder / "tokenizer_config.json").write_text(json.dumps({
        "tokenizer_class": "Qwen2Tokenizer", "model_max_length": 1536,
        "eos_token": "<|im_end|>", "pad_token": "<|endoftext|>",
        "added_tokens_decoder": {str(words[s]): {"content": s, "special": True, "normalized": False,
                                               "lstrip": False, "rstrip": False, "single_word": False} for s in specials},
    }))
    (folder / "chat_template.jinja").write_text(
        "{% for m in messages %}{{ '<|im_start|>' + m['role'] + '\\n' + m['content'] + '<|im_end|>\\n' }}{% endfor %}"
        "{% if add_generation_prompt %}{{ '<|im_start|>assistant\\n<think>\\n' }}{% endif %}")
    assert made.encode("<|im_start|>user\n").ids == [248045, 846, 198]


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "pan2"
    torch.manual_seed(0)
    tokenizer(out / "tokenizer")
    text = Qwen3_5TextConfig(
        vocab_size=248320, hidden_size=32, intermediate_size=64, num_hidden_layers=4,
        num_attention_heads=2, num_key_value_heads=1, head_dim=16,
        linear_conv_kernel_dim=4, linear_key_head_dim=8, linear_value_head_dim=8,
        linear_num_key_heads=2, linear_num_value_heads=2,
        layer_types=["linear_attention"] * 3 + ["full_attention"],
        rope_parameters={"rope_type": "default", "rope_theta": 10000.0, "partial_rotary_factor": 0.25},
    )
    save(module(Qwen3_5TextModel(text)), out / "text_encoder")
    save(module(PAN2Transformer3DModel(
        in_channels=97, out_channels=48, patch_size=(1, 2, 2), num_layers=2, num_refiner_layers=1,
        attention_head_dim=16, num_attention_heads=2, text_embed_dim=32, mlp_ratio=2.0,
        context_mlp_ratio=2.0, refiner_mlp_ratio=2.0, rope_axes_dim=(4, 6, 6),
        rope_theta=256.0, rope_temporal_scale=2 / 3,
    )), out / "transformer")
    save(module(AutoencoderKLWan(
        base_dim=3, dim_mult=[1, 1, 1, 1], num_res_blocks=1, z_dim=48, in_channels=12,
        out_channels=12, patch_size=2, is_residual=True, temperal_downsample=[False, True, True],
        scale_factor_spatial=16, scale_factor_temporal=4, latents_mean=[0.0] * 48, latents_std=[1.0] * 48,
    )), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(shift=9), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "PAN2ModularPipeline", "_diffusers_version": "0.43.0",
        "transformer": ["diffusers", "PAN2Transformer3DModel"],
        "text_encoder": ["transformers", "Qwen3_5TextModel"], "vae": ["diffusers", "AutoencoderKLWan"],
        "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"], "tokenizer": ["transformers", "Qwen2Tokenizer"],
    }))
    return {REF: out, **qwen3_5.models(root, store)}


_SMALL: dict[str, Json] = {"prompt": _CAPTION, "duration_s": 1, "seed": 7, "steps": 1,
                           "guidance": 1.0, "prompt_upsampling": False}
CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": dict(_SMALL), "check": video(1248, 704, 25)},
    {"entry": "generate", "name": "cfg", "payload": {**_SMALL, "guidance": 2.5}, "check": video(1248, 704, 25)},
    {"entry": "generate", "name": "first-frame", "payload": {**_SMALL, "aspect_ratio": "16:9", "guidance": 2.5},
     "files": {"first_frame": _shared.picture("opening", 96, 64)}, "check": video(1248, 704, 25)},
]
