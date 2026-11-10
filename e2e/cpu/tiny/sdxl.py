"""Tiny SDXL: the pipeline's real classes at a few channels, as a Diffusers folder in half precision."""

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKL, EulerDiscreteScheduler, UNet2DConditionModel
from transformers import CLIPTextConfig, CLIPTextModel, CLIPTextModelWithProjection

from cell import Cell, module, save, size


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "sdxl"
    torch.manual_seed(0)
    clip = CLIPTextConfig(hidden_size=32, intermediate_size=64, num_attention_heads=4, num_hidden_layers=3,
                          vocab_size=49408, max_position_embeddings=77, projection_dim=32, hidden_act="gelu")
    save(module(CLIPTextModel(clip)).half(), out / "text_encoder")
    save(module(CLIPTextModelWithProjection(clip)).half(), out / "text_encoder_2")
    # SDXL's own topology (HiDiffusion recognizes the UNet by its module names), a few channels wide.
    save(module(UNet2DConditionModel(
        block_out_channels=(32, 64, 128), layers_per_block=2, sample_size=128, in_channels=4, out_channels=4,
        down_block_types=("DownBlock2D", "CrossAttnDownBlock2D", "CrossAttnDownBlock2D"),
        up_block_types=("CrossAttnUpBlock2D", "CrossAttnUpBlock2D", "UpBlock2D"),
        attention_head_dim=(2, 4, 8), use_linear_projection=True, addition_embed_type="text_time",
        addition_time_embed_dim=8, transformer_layers_per_block=(1, 2, 10), projection_class_embeddings_input_dim=80,
        cross_attention_dim=64, norm_num_groups=8,
    )).half(), out / "unet")
    save(module(AutoencoderKL(
        block_out_channels=(8, 8, 16, 16), in_channels=3, out_channels=3, down_block_types=("DownEncoderBlock2D",) * 4,
        up_block_types=("UpDecoderBlock2D",) * 4, latent_channels=4, norm_num_groups=4, layers_per_block=1,
        sample_size=1024, scaling_factor=0.13025,
    )).half(), out / "vae")
    save(EulerDiscreteScheduler(beta_start=0.00085, beta_end=0.012, beta_schedule="scaled_linear",
                                timestep_spacing="leading", steps_offset=1), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "StableDiffusionXLPipeline", "_diffusers_version": "0.40.0",
        "scheduler": ["diffusers", "EulerDiscreteScheduler"], "text_encoder": ["transformers", "CLIPTextModel"],
        "text_encoder_2": ["transformers", "CLIPTextModelWithProjection"],
        "unet": ["diffusers", "UNet2DConditionModel"], "vae": ["diffusers", "AutoencoderKL"]}))
    return {"stabilityai/sdxl@1.0.0": out}


CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"prompt": "a red fox", "steps": 2, "seed": 7}, "check": size(1024, 1024)},
    {"entry": "generate", "name": "no-cfg", "payload": {"prompt": "a red fox", "steps": 2, "seed": 7, "guidance": 1.0}},
    {"entry": "generate", "name": "wide", "payload": {"prompt": "a red fox", "steps": 2, "seed": 7, "aspect_ratio": "16:9"},
     "check": size(1344, 768)},
    {"entry": "generate", "name": "hidiffusion", "payload": {"prompt": "a red fox", "steps": 2, "seed": 7, "hidiffusion": True}},
    {"entry": "image", "name": "tool", "payload": {"instructions": "a red fox", "seed": 7, "aspect_ratio": "1:1"}},
]
