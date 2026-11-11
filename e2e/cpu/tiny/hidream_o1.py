"""Tiny HiDream-O1: the package's own `Root` (Qwen3-VL's text model and vision tower with HiDream's patch embedder,
timestep token and pixel head) at a few channels, as HiDream's release lays it out: `config.json`, `model.safetensors`
(`model.*`), Qwen's tokenizer with HiDream's two tokens, and the Qwen3-VL processor."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import numpy as np
import torch
from hidream_o1.model import BOI, TMS, Root
from PIL import Image
from safetensors.torch import save_file
from transformers import AutoTokenizer, Qwen3VLConfig

from cell import Cell, Json, module, packages, quantize, qwen_tokenizer, save

#: Qwen3-VL's text width here; HiDream's patch embedder narrows a 32x32 patch to a quarter of it.
WIDTH = 64


def config() -> Qwen3VLConfig:
    return Qwen3VLConfig(
        text_config={"hidden_size": WIDTH, "intermediate_size": 128, "num_attention_heads": 4, "num_key_value_heads": 2,
                     "head_dim": 16, "num_hidden_layers": 2, "vocab_size": 151936, "rope_theta": 5000000,
                     "rope_scaling": {"mrope_interleaved": True, "mrope_section": [4, 2, 2], "rope_type": "default"}},
        vision_config={"depth": 1, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2, "out_hidden_size": WIDTH,
                       "deepstack_visual_indexes": [0], "patch_size": 16, "spatial_merge_size": 2, "temporal_patch_size": 2},
        image_token_id=151655, video_token_id=151656, vision_start_token_id=151652, vision_end_token_id=151653,
    )


def write(folder: Path) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    root = module(Root(config()))
    for parameter in root.parameters():
        parameter.data = parameter.data.to(torch.bfloat16)
    save_file({k: v.contiguous() for k, v in root.state_dict().items()}, folder / "model.safetensors",
              metadata={"format": "pt"})
    document = config().to_dict()
    document["architectures"] = ["Qwen3VLForConditionalGeneration"]
    (folder / "config.json").write_text(json.dumps(document))
    qwen_tokenizer(folder)
    tokenizer = AutoTokenizer.from_pretrained(folder)
    tokenizer.add_special_tokens({"additional_special_tokens": [BOI, TMS]})
    save(tokenizer, folder)
    processor = packages() / "minimax-h3" / "minimax_h3" / "processor"
    for name in ("preprocessor_config.json", "video_preprocessor_config.json"):
        shutil.copyfile(processor / name, folder / name)
    return folder


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    full = write(root / "full")
    torch.manual_seed(1)
    dev = write(root / "dev")
    return {"hidream-ai/hidream-o1@0.1.0": full, "hidream-ai/hidream-o1-dev@0.1.0": dev}


def fox(folder: Path) -> Path:
    y, x = np.mgrid[0:256, 0:320].astype(np.float32)
    path = folder / "fox.png"
    Image.fromarray(np.stack([x * 0.8, y, np.where((x - 160) ** 2 + (y - 128) ** 2 < 4000, 220.0, 60.0)], -1)
                    .astype(np.uint8)).save(path)
    return path


_SMALL: dict[str, Json] = {"prompt": {"text": "a red fox on a mossy log"}, "steps": 2, "seed": 7}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": _SMALL},
    {"entry": "generate", "name": "reference", "payload": {**_SMALL, "prompt": {"text": "<fox> on a mossy log",
                                                                                 "references": [{"key": "fox"}]}},
     "files": {"prompt.references.0.file": fox}},
    {"entry": "generate_turbo", "name": "text", "payload": {"prompt": {"text": "a red fox"}, "seed": 7, "aspect_ratio": "16:9"}},
    {"entry": "image", "name": "tool", "payload": {"instructions": {"text": "a red fox"}, "seed": 7}},
    quantize("hidream-ai/hidream-o1@0.1.0", "bf16", "hidream-o1", "hidream-o1@0.6.1"),
]
