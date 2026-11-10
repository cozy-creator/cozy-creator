"""Tiny Qwen-Image-2.1: the Diffusers folder Qwen ships (the causal-condition DiT, Qwen3-VL as its text encoder with
Qwen3-VL's processor, the 16x RGBA VAE in F32) at a few channels, and Qwen's two prompt enhancers (Qwen3.5 VLMs, as-is
uploads) at 32 wide. The DiT keeps its 32 blocks: the graph hooks the overlay's sites on every one (`model.py` SITES).
The abliterated text encoder and both enhancers are bound to these same tiny weights.

Alibaba PAI's Fun Acc overlay builds at the release geometry (`model.py` SITES: 4096 wide, `turbo.py` FunAccGraph), which
no tiny DiT matches, and `generate_turbo` loads it with the function: Turbo is GPU-only."""

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


def _image(out: Path) -> None:
    _shared.qwen_processor(out / "processor")
    save(module(Qwen3VLForConditionalGeneration(_shared.qwen3_vl(2, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    save(module(QwenImage21Transformer2DModel(
        patch_size=1, in_channels=Z, out_channels=Z, num_layers=32, attention_head_dim=8, num_attention_heads=2,
        context_in_dim=TEXT_WIDTH, axes_dims_rope=(2, 2, 4), causal_condition=True,
    )).to(torch.bfloat16), out / "transformer")
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
    """Fun Acc's layout (TensorFS's `qwenimage21.lora`: PEFT factors and the per-step output projections) over the
    tiny DiT's first block, rank 4."""
    rank, inner = 4, 16
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("transformer", cls=None, config={"r": rank, "lora_alpha": rank})
    generator = torch.Generator().manual_seed(2)
    for key, shape in {"transformer_blocks.0.attn.to_q.lora_A.weight": [rank, inner],
                       "transformer_blocks.0.attn.to_q.lora_B.weight": [inner, rank],
                       "proj_out.heads": [4, Z, inner]}.items():
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
_FUN_ACC = "Fun Acc builds at the release geometry (model.py SITES, turbo.py FunAccGraph); no tiny DiT takes it"

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
    {"entry": "generate_turbo", "name": "text", "payload": {}, "skip": _FUN_ACC + "; generate_turbo loads it with the function"},
    {"entry": "image", "name": "turbo", "payload": {}, "skip": _FUN_ACC},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
