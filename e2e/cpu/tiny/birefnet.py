"""BiRefNet at its own size (the package builds `BiRefNet(BiRefNetConfig(bb_pretrained=False))`, no smaller config
exists), random half-precision weights, as ZhengPeng7's release lays it out: `config.json` and `model.safetensors`."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch
from birefnet.upstream.BiRefNet_config import BiRefNetConfig
from birefnet.upstream.birefnet import BiRefNet
from PIL import Image
from safetensors.torch import save_file

from cell import Cell, module


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    folder = root / "birefnet"
    folder.mkdir(parents=True)
    model = module(BiRefNet(config=BiRefNetConfig(bb_pretrained=False))).half()
    save_file({k: v.contiguous() for k, v in model.state_dict().items()}, folder / "model.safetensors",
              metadata={"format": "pt"})
    (folder / "config.json").write_text(json.dumps({"architectures": ["BiRefNet"], "bb_pretrained": False,
                                                    "model_type": "SwinTransformer"}))
    return {"zhengpeng7/birefnet@0.1.0": folder}


def character(folder: Path) -> Path:
    y, x = np.mgrid[0:384, 0:512].astype(np.float32)
    subject = ((x - 256) ** 2 / 120**2 + (y - 200) ** 2 / 150**2) < 1
    rgb = np.where(subject[..., None], [200.0, 80.0, 40.0], np.stack([x / 2, y / 2, np.full_like(x, 90)], -1))
    path = folder / "character.png"
    Image.fromarray(rgb.astype(np.uint8)).save(path)
    return path


CELLS: list[Cell] = [
    {"entry": "remove_background", "name": "image", "payload": {}, "files": {"image": character}},
]
