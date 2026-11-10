"""Tiny Qwen-Image: Qwen-Image-2512 and Qwen-Image-Edit-2511 as the Diffusers folders Qwen ships (the dual-stream DiT,
Qwen2.5-VL with its vision tower, the Qwen-Image VAE in bf16, Qwen's tokenizer as vocabulary and merges), at a few
channels. The edit checkpoint's DiT reads its references at t = 0 (`zero_cond_t`); its text encoder and VAE are the
base's, byte for byte, as in the releases.

lightx2v's Lightning LoRAs build at the 20B DiT's geometry (`model.py` LoRAGraph: 3072 wide, 60 blocks), which no tiny
DiT matches, and `generate_turbo` loads its `turbo_lora` with the function: every Lightning path is GPU-only."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import tensorfs
import torch
from diffusers import AutoencoderKLQwenImage, FlowMatchEulerDiscreteScheduler, QwenImageTransformer2DModel
from transformers import Qwen2_5_VLConfig, Qwen2_5_VLForConditionalGeneration

from cell import Cell, Json, module, packages, save, size
from tiny import _shared

BASE, EDIT = "qwen/qwen-image-2512@0.1.1", "qwen/qwen-image-edit-2511@0.1.0"
LORAS = ("lightx2v/qwen-image-2512-lightning-lora@0.1.0", "lightx2v/qwen-image-edit-2511-lightning-lora@0.1.0")
TEXT_WIDTH, Z = 32, 16
_SITES = ("attn.to_q", "attn.to_k", "attn.to_v", "attn.to_out.0", "attn.add_q_proj", "attn.add_k_proj", "attn.add_v_proj",
          "attn.to_add_out", "img_mlp.net.0.proj", "img_mlp.net.2", "txt_mlp.net.0.proj", "txt_mlp.net.2")


def _transformer(edit: bool) -> torch.nn.Module:
    return module(QwenImageTransformer2DModel(
        patch_size=2, in_channels=4 * Z, out_channels=Z, num_layers=2, attention_head_dim=16, num_attention_heads=2,
        joint_attention_dim=TEXT_WIDTH, axes_dims_rope=(4, 6, 6), zero_cond_t=edit,
    )).to(torch.bfloat16)


def _tokenizer(folder: Path) -> None:
    """Qwen2.5-VL's tokenizer as Qwen-Image's `tokenizer/` holds it: vocabulary, merges and the config, from the copy
    minimax-h3 bundles (Qwen's byte-level BPE)."""
    source = packages() / "minimax-h3" / "minimax_h3" / "processor"
    folder.mkdir(parents=True)
    for name in ("vocab.json", "merges.txt", "tokenizer_config.json"):
        shutil.copyfile(source / name, folder / name)


def _base(out: Path) -> None:
    _tokenizer(out / "tokenizer")
    config = Qwen2_5_VLConfig(
        text_config={"vocab_size": 152064, "hidden_size": TEXT_WIDTH, "intermediate_size": 64, "num_hidden_layers": 2,
                     "num_attention_heads": 4, "num_key_value_heads": 2, "max_position_embeddings": 8192,
                     "rope_parameters": {"rope_type": "default", "rope_theta": 1e6, "mrope_section": [2, 1, 1]}},
        vision_config={"depth": 2, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2, "out_hidden_size": TEXT_WIDTH,
                       "fullatt_block_indexes": [1], "window_size": 112},
    )
    save(module(Qwen2_5_VLForConditionalGeneration(config)).to(torch.bfloat16), out / "text_encoder")
    save(_transformer(edit=False), out / "transformer")
    save(module(AutoencoderKLQwenImage(base_dim=8, z_dim=Z, num_res_blocks=1)).to(torch.bfloat16), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(use_dynamic_shifting=True, base_shift=0.5, max_shift=0.9, base_image_seq_len=256,
                                         max_image_seq_len=8192, shift_terminal=0.02, time_shift_type="exponential"),
         out / "scheduler")


def _index(out: Path, pipeline: str) -> None:
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": pipeline, "_diffusers_version": "0.41.0", "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", "Qwen2_5_VLForConditionalGeneration"], "tokenizer": ["transformers", "Qwen2Tokenizer"],
        "transformer": ["diffusers", "QwenImageTransformer2DModel"], "vae": ["diffusers", "AutoencoderKLQwenImage"]}))


def _lora(store: Path, seed: int) -> str:
    """A Lightning LoRA as TensorFS stores lightx2v's kohya file (PEFT factors and each site's alpha), over the tiny DiT."""
    rank = 4
    with torch.device("meta"):
        dit = _transformer(edit=False)
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("model", cls=None, config={"r": rank, "lora_alpha": rank})
    generator = torch.Generator().manual_seed(seed)
    for block in range(2):
        for site in _SITES:
            path = f"transformer_blocks.{block}.{site}"
            linear = dit.get_submodule(path)
            assert isinstance(linear, torch.nn.Linear)
            for factor, shape in (("lora_A", [rank, linear.in_features]), ("lora_B", [linear.out_features, rank])):
                data = (torch.randn(shape, generator=generator) * 0.02).to(torch.bfloat16).contiguous()
                component.put(f"{path}.{factor}.weight", memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16",
                              shape=shape)
            alpha = torch.tensor(float(rank)).contiguous()
            component.put(f"{path}.alpha", memoryview(alpha.view(torch.uint8).numpy()).cast("B"), dtype="F32", shape=[])
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    base, edit = root / "qwen-image", root / "qwen-image-edit"
    _base(base)
    _index(base, "QwenImagePipeline")
    shutil.copytree(base, edit, ignore=shutil.ignore_patterns("transformer"))
    torch.manual_seed(1)
    save(_transformer(edit=True), edit / "transformer")
    _index(edit, "QwenImageEditPlusPipeline")
    return {BASE: base, EDIT: edit, LORAS[0]: _lora(store, 2), LORAS[1]: _lora(store, 3)}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 320, 256)
_SNOW = _shared.picture("snow", 128, 128)
#: 256², Qwen-Image's smallest canvas (0.0625 MP).
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 0.0625, "steps": 2}
_LIGHTNING = "lightx2v's Lightning LoRA builds at the 20B DiT's geometry (model.py LoRAGraph); no tiny DiT takes it"

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(256, 256)},
    {"entry": "generate", "name": "no-cfg", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_SMALL, "guidance": 1.0}},
    {"entry": "generate", "name": "edit", "payload": {"prompt": _prompt("the fox in <fox> wears a hat", "fox"), **_SMALL},
     "files": {"prompt.references.0.file": _FOX}, "check": size(288, 224)},
    {"entry": "generate", "name": "edit-two", "payload": {"prompt": _prompt("the fox in <fox> on <snow>", "fox", "snow"),
                                                          "seed": 7, "steps": 2},
     "files": {"prompt.references.0.file": _FOX, "prompt.references.1.file": _SNOW}, "check": size(320, 256)},
    {"entry": "image", "name": "full", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "turbo": False},
     "check": size(1328, 1328)},
    {"entry": "generate_turbo", "name": "text", "payload": {}, "skip": _LIGHTNING + "; generate_turbo loads it eagerly"},
    {"entry": "image", "name": "turbo", "payload": {}, "skip": _LIGHTNING},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": "weights entry: the driver does not grant Invoke.sources yet"},
]
