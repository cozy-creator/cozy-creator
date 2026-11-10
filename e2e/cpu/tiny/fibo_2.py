"""Tiny Fibo 2.0: Dev and Edit Turbo as Bria's Diffusers folders (`BriaFibo2Transformer2DModel` with its five injection
blocks and a small gist, Qwen3-VL with Qwen3-VL's processor in `processor/`, FLUX.2's VAE in F32) at a few channels, and
Bria's prompt-to-JSON VLMs (Qwen3-VL as-is uploads, their head tied to the embedding as the 4B's is). Qwen3-VL keeps the
35 layers the bundles read (5 ... 35), 32 wide."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKLFlux2, BriaFibo2Transformer2DModel, FlowMatchEulerDiscreteScheduler
from transformers import Qwen3VLForConditionalGeneration

from cell import Cell, Json, module, save, size
from tiny import _shared

DEV, TURBO = "briaai/fibo-2-dev@0.1.0", "briaai/fibo-2-edit-turbo@0.1.0"
VLM, EDIT_VLM = "briaai/fibo-vlm@0.1.0", "briaai/fibo-edit-vlm@0.1.0"
TEXT_WIDTH = 32


def _fibo(out: Path) -> None:
    _shared.qwen_processor(out / "processor")
    save(module(Qwen3VLForConditionalGeneration(_shared.qwen3_vl(35, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    save(module(BriaFibo2Transformer2DModel(
        in_channels=4, patch_size=2, dim=32, n_layers=5, n_refiner_layers=1, n_heads=2, cap_feat_dim=3 * TEXT_WIDTH,
        injection_layer_ids=(0, 1, 2, 3, 4), perceiver_num_layers=1, min_num_gist_tokens=16, max_num_gist_tokens=32,
        gist_step=8, gist_min_text_len=16, gist_max_text_len=128, axes_dims=(4, 6, 6),
    )).to(torch.bfloat16), out / "transformer")
    save(module(AutoencoderKLFlux2(block_out_channels=(8, 8, 16, 16), layers_per_block=1, latent_channels=4,
                                   norm_num_groups=4)), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(shift=3.0, use_dynamic_shifting=True), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "BriaFibo2Pipeline", "_diffusers_version": "0.38.0.dev0", "processor": ["transformers", "Qwen3VLProcessor"],
        "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", "Qwen3VLForConditionalGeneration"],
        "transformer": ["diffusers", "BriaFibo2Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLFlux2"]}))


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    _fibo(root / "fibo-2-dev")
    torch.manual_seed(1)
    _fibo(root / "fibo-2-edit-turbo")
    torch.manual_seed(2)
    vlm = root / "fibo-vlm"
    save(module(Qwen3VLForConditionalGeneration(_shared.qwen3_vl(2, TEXT_WIDTH, tie_word_embeddings=True))).to(torch.bfloat16),
         vlm)
    _shared.qwen_processor(vlm)
    return {DEV: root / "fibo-2-dev", TURBO: root / "fibo-2-edit-turbo", VLM: vlm, EDIT_VLM: vlm}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 640, 512)
_SNOW = _shared.picture("snow", 512, 512)
_CAPTION = json.dumps({"short_description": "a red fox in fresh snow", "objects": [{"description": "a red fox"}],
                       "background_setting": "a snowy field", "text_render": [{"text": "SNOW", "location": "bottom"}]})
#: 512², Fibo's smallest canvas (0.25 MP), 2 steps, the prompt as written.
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 0.25, "steps": 2, "prompt_upsampling": False}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(512, 512)},
    {"entry": "generate", "name": "caption", "payload": {"prompt": _prompt(_CAPTION), **_SMALL}},
    {"entry": "generate", "name": "upsampled", "payload": {"prompt": _prompt("a red fox"), **_SMALL, "prompt_upsampling": True}},
    {"entry": "generate", "name": "edit", "payload": {"prompt": _prompt("give <fox> a hat", "fox"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "generate_turbo", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(512, 512)},
    {"entry": "generate_turbo", "name": "edit-upsampled",
     "payload": {"prompt": _prompt("give <fox> a hat", "fox"), **_SMALL, "prompt_upsampling": True},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "generate_turbo", "name": "two-edits", "payload": {"prompt": _prompt("<fox> on <snow>", "fox", "snow"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX, "prompt.references.1.file": _SNOW}},
    {"entry": "image", "name": "turbo", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "full", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "turbo": False,
                                                  "prompt_upsampling": False}, "check": size(1024, 1024)},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": "weights entry: the driver does not grant Invoke.sources yet"},
]
