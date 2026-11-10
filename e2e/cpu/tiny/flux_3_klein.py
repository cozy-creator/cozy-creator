"""Tiny FLUX.3 Klein: the distilled flow map and Base as BFL's Diffusers folders (`Flux3Transformer2DModel`, Qwen3-VL
with its processor files in `text_encoder/`, FLUX.2's VAE in F32) at a few channels, the upsampler (a Qwen3.5 VLM, an
as-is upload) and the drafter (Qwen3.5's multi-token-prediction head alone, as `drafter_head` writes it, Runtime's
`lm.Draft`). Qwen3-VL keeps the 32 layers Klein taps (4, 8, ..., 32), 32 wide."""

from __future__ import annotations

import json
from pathlib import Path

import tensorfs
import torch
from cozy_runtime import lm
from diffusers import AutoencoderKLFlux2, Flux3Transformer2DModel
from transformers import Qwen3_5ForConditionalGeneration, Qwen3VLForConditionalGeneration
from transformers.models.qwen3_5 import modeling_qwen3_5

from cell import NARROW, Cell, Json, module, save, size
from tiny import _shared

DISTILLED, BASE = "black-forest-labs/flux-3-klein@0.1.0", "black-forest-labs/flux-3-klein-base@0.1.0"
UPSAMPLER, DRAFTER = "atomicchat/flux-3-klein-upsampler-abliterated@0.1.0", "qwen/qwen3.5-9b-mtp@0.2.0"
TEXT_WIDTH = 32


def _klein(out: Path, flow_map: bool) -> None:
    save(module(Qwen3VLForConditionalGeneration(_shared.qwen3_vl(32, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    _shared.qwen_processor(out / "text_encoder")
    save(module(Flux3Transformer2DModel(
        in_channels=16, cond_in_channels=16, num_layers=1, num_single_layers=1, attention_head_dim=8, num_attention_heads=2,
        joint_attention_dim=8 * TEXT_WIDTH, timestep_channels=32, axes_dims_rope=(2, 2, 2, 2), flow_map=flow_map,
    )).to(torch.bfloat16), out / "transformer")
    save(module(AutoencoderKLFlux2(block_out_channels=(8, 8, 16, 16), layers_per_block=1, latent_channels=4,
                                   norm_num_groups=4)), out / "vae")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "Flux3KleinPipeline", "_diffusers_version": "0.38.0.dev0",
        "text_encoder": ["transformers", "Qwen3VLForConditionalGeneration"],
        "transformer": ["diffusers", "Flux3Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLFlux2"]}))


def _drafter(store: Path) -> str:
    """The head-only lane: Qwen3.5's config, and the `mtp.*` weights of `lm.Draft` at it."""
    config = _shared.qwen3_5_vl(mtp_num_hidden_layers=1)
    head = lm.Draft(config, modeling_qwen3_5).to(torch.bfloat16)
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("model", cls=None, config=json.loads(config.to_json_string()))
    for key, tensor in head.state_dict().items():
        data = tensor.detach().contiguous()
        component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16", shape=list(data.shape))
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    _klein(root / "klein", flow_map=True)
    torch.manual_seed(1)
    _klein(root / "klein-base", flow_map=False)
    torch.manual_seed(2)
    upsampler = root / "upsampler"
    save(module(Qwen3_5ForConditionalGeneration(_shared.qwen3_5_vl())).to(torch.bfloat16), upsampler)
    _shared.qwen_processor(upsampler)
    torch.manual_seed(3)
    return {DISTILLED: root / "klein", BASE: root / "klein-base", UPSAMPLER: upsampler, DRAFTER: _drafter(store)}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 200, 136)
_SNOW = _shared.picture("snow", 128, 128)
#: 128², 2 steps, the prompt as written.
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 128 * 128 / (1 << 20), "steps": 2, "prompt_upsampling": False}
_SIGN = [{"box": [0.55, 0.6, 0.95, 0.9], "prompt": "a wooden sign", "text": "SNOW"}]

CELLS: list[Cell] = [
    {"entry": "generate_turbo", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(128, 128)},
    {"entry": "generate_turbo", "name": "upsampled", "payload": {"prompt": _prompt("a red fox"), **_SMALL,
                                                                 "prompt_upsampling": True}},
    {"entry": "generate_turbo", "name": "upsampled-plain", "payload": {"prompt": _prompt("a red fox"), **_SMALL,
                                                                       "prompt_upsampling": True, "prompt_drafting": False}},
    {"entry": "generate_turbo", "name": "reference", "payload": {"prompt": _prompt("the fox in <fox> wears a hat", "fox"),
                                                                 "seed": 7, "steps": 2, "prompt_upsampling": False},
     "files": {"prompt.references.0.file": _FOX}, "check": size(200, 136)},
    {"entry": "generate_turbo", "name": "placed-upsampled",
     "payload": {"prompt": _prompt("<fox> on <snow>", "fox", "snow"), **_SMALL, "prompt_upsampling": True,
                 "boxes": {"fox": [0.1, 0.2, 0.5, 0.8]}},
     "files": {"prompt.references.0.file": _FOX, "prompt.references.1.file": _SNOW}, "check": size(128, 128)},
    {"entry": "generate_turbo", "name": "regions", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL,
                                                               "regions": _SIGN}},
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL,
                                                      "negative_prompt": "blurry"}, "check": size(128, 128)},
    {"entry": "generate", "name": "two-references",
     "payload": {"prompt": _prompt("the fox in <fox> on <snow>", "fox", "snow"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX, "prompt.references.1.file": _SNOW}},
    {"entry": "image", "name": "turbo", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "full", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "turbo": False,
                                                  "prompt_upsampling": False}, "check": size(1024, 1024)},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
