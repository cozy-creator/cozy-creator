"""Tiny Qwen-Image-2.1: the Diffusers folder Qwen ships (the causal-condition DiT, Qwen3-VL as its text encoder with
Qwen3-VL's processor, the 16x RGBA VAE in F32) at a few channels, and Qwen's two prompt enhancers (Qwen3.5 VLMs, as-is
uploads) at 32 wide. The DiT has 2 blocks; Alibaba PAI's Fun Acc overlay builds at the release geometry (`model.py`
SITES: 4096 wide, 32 blocks), so the executor takes the tiny DiT's (PATCH). The abliterated text encoder and both
enhancers are bound to these same tiny weights."""

from __future__ import annotations

import json
from pathlib import Path

import tensorfs
import torch
from diffusers import AutoencoderKLQwenImage21, FlowMatchEulerDiscreteScheduler, QwenImage21Transformer2DModel
from transformers import Qwen3_5ForConditionalGeneration, Qwen3VLForConditionalGeneration

from cell import NARROW, Cell, Json, module, save, size
from tiny import _shared

MODEL, ENCODER = "qwen/qwen-image-2.1@0.1.0", "atomicchat/qwen-image-2.1-text-encoder-abliterated@0.1.0"
ENHANCERS = ("pottokao/qwen-image-2.1-pe-t2i-abliterated@0.1.0", "darrellbest/qwen-image-2.1-pe-i2i-abliterated@0.1.0")
FUN_ACC = "alibaba-pai/qwen-image-2.1-fun-acc@0.1.0"
TEXT_WIDTH, Z = 32, 64


def _dit() -> torch.nn.Module:
    return module(QwenImage21Transformer2DModel(
        patch_size=1, in_channels=Z, out_channels=Z, num_layers=2, attention_head_dim=8, num_attention_heads=2,
        context_in_dim=TEXT_WIDTH, axes_dims_rope=(2, 2, 4), causal_condition=True,
    ))


def _sites() -> tuple[dict[str, tuple[int, int]], tuple[int, int]]:
    """`model.SITES` (each adapted linear's in and out) and `PROJ_OUT` (out, in) as the tiny DiT has them: the release's
    names, its 4096-wide 32 blocks shrunk."""
    with torch.device("meta"):
        dit = _dit()
    names = ["img_in", "txt_in.in_layer", "txt_in.out_layer", "modulation.1", "time_text_embed.timestep_embedder.linear_1",
             "time_text_embed.timestep_embedder.linear_2", "norm_out.linear",
             *(f"transformer_blocks.{i}.{site}" for i in range(2) for site in
               ("attn.to_q", "attn.to_k", "attn.to_v", "attn.to_out.0", "img_mlp.gate_layer", "img_mlp.proj", "img_mlp.out"))]
    sites: dict[str, tuple[int, int]] = {}
    for name in names:
        linear = dit.get_submodule(name)
        assert isinstance(linear, torch.nn.Linear), name
        sites[name] = (linear.in_features, linear.out_features)
    head = dit.get_submodule("proj_out")
    assert isinstance(head, torch.nn.Linear)
    return sites, (head.out_features, head.in_features)


SITES, PROJ_OUT = _sites()
#: The Fun Acc overlay's sites (`model.SITES`, `PROJ_OUT`) at the tiny DiT's geometry: the release's stay GPU-only.
PATCH: dict[str, object] = {"qwen_image_2.model:SITES": SITES, "qwen_image_2.model:PROJ_OUT": PROJ_OUT}


def _image(out: Path) -> None:
    _shared.qwen_processor(out / "processor")
    save(module(Qwen3VLForConditionalGeneration(_shared.qwen3_vl(2, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    save(_dit().to(torch.bfloat16), out / "transformer")
    save(module(AutoencoderKLQwenImage21(base_dim=8, decoder_base_dim=8, z_dim=Z, num_res_blocks=1)), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(use_dynamic_shifting=True, base_shift=0.5, max_shift=0.9, base_image_seq_len=256,
                                         max_image_seq_len=8192, shift_terminal=0.02, time_shift_type="exponential"),
         out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "QwenImage21Pipeline", "_diffusers_version": "0.41.0", "processor": ["transformers", "Qwen3VLProcessor"],
        "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", "Qwen3VLForConditionalGeneration"],
        "transformer": ["diffusers", "QwenImage21Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLQwenImage21"]}))


def _enhancer(out: Path) -> None:
    """A Qwen3.5 VLM as its as-is upload: the Transformers folder with its processor files at the root."""
    save(module(Qwen3_5ForConditionalGeneration(_shared.qwen3_5_vl())).to(torch.bfloat16), out)
    _shared.qwen_processor(out)


def _fun_acc(store: Path) -> str:
    """Fun Acc's layout (TensorFS's `qwenimage21.lora`): rank-4 factors on every adapted linear and the four per-step
    output projections (`proj_out.heads`)."""
    rank = 4
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("transformer", cls=None, config={"r": rank, "lora_alpha": rank})
    generator = torch.Generator().manual_seed(2)
    shapes = {f"{site}.{factor}.weight": shape for site, (inputs, outputs) in SITES.items()
              for factor, shape in (("lora_A", [rank, inputs]), ("lora_B", [outputs, rank]))}
    for key, shape in {**shapes, "proj_out.heads": [4, *PROJ_OUT]}.items():
        data = (torch.randn(shape, generator=generator) * 0.02).to(torch.bfloat16).contiguous()
        component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16", shape=shape)
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    _image(root / "qwen-image-2.1")
    torch.manual_seed(1)
    _enhancer(root / "enhancer")
    return {MODEL: root / "qwen-image-2.1", ENCODER: root / "qwen-image-2.1", ENHANCERS[0]: root / "enhancer",
            ENHANCERS[1]: root / "enhancer", FUN_ACC: _fun_acc(store)}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 320, 256)
#: 1 MP, the least Qwen-Image-2.1 serves, at its fixed 40 steps.
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 1, "prompt_upsampling": False}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(1024, 1024)},
    {"entry": "generate", "name": "upsampled", "payload": {"prompt": _prompt("a red fox"), **_SMALL, "prompt_upsampling": True}},
    {"entry": "generate", "name": "edit", "payload": {"prompt": _prompt("the fox in <fox> wears a hat", "fox"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "generate", "name": "edit-upsampled",
     "payload": {"prompt": _prompt("the fox in <fox> wears a hat", "fox"), **_SMALL, "prompt_upsampling": True,
                 "creativity": "faithful"},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "image", "name": "full", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "turbo": False,
                                                  "megapixels": 1, "creativity": "creative"}},
    {"entry": "generate_turbo", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(1024, 1024)},
    {"entry": "generate_turbo", "name": "edit", "payload": {"prompt": _prompt("the fox in <fox> wears a hat", "fox"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX}},
    {"entry": "image", "name": "turbo", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7,
                                                   "megapixels": 1}},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
