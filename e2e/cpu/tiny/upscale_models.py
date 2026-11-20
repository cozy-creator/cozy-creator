"""upscale-models: tiny DATs, spandrel's own class at 16 channels in one group of two blocks (spatial, then channel
attention), random F32 weights, each saved as the Hub's mirror of a network is: one `model.safetensors`, the uploaded
file's component `model`, its keys spandrel's. The default's (IllustrationJaNai V3detail) has DAT2's pixelshuffle tail and
its 2x4 window; the photo network's slot holds the light pixelshuffledirect tail and a 4x8 window, so both of the package's
scale readings and two windows run. Every cell asks 1080p: a 480 px pass in one tile (×4), a 960 px pass in four even tiles
(×2, the ×4 pass brought down by Lanczos), and two passes from 240 px, the second tiled."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import numpy as np
import torch
from PIL import Image
from safetensors.torch import save_file
from spandrel.architectures.DAT import DAT
from upscale_models import ART, PHOTO

from cell import Cell, defined, size


def _network(folder: Path, split: list[int], upsampler: str) -> Path:
    folder.mkdir(parents=True)
    with defined():
        network = DAT(img_size=64, in_chans=3, embed_dim=16, split_size=split, depth=[2], num_heads=[2],
                      expansion_factor=2.0, upscale=4, resi_connection="1conv", upsampler=upsampler)
    state: dict[str, torch.Tensor] = network.state_dict()
    save_file({k: v.contiguous() for k, v in state.items()}, folder / "model.safetensors", metadata={"format": "pt"})
    return folder


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    return {ART: _network(root / "art", [2, 4], "pixelshuffle"),
            PHOTO: _network(root / "photo", [4, 8], "pixelshuffledirect")}


def picture(width: int, height: int) -> Callable[[Path], Path]:
    """Edges, rings and a gradient, so a seam or a wrong window shows."""

    def make(folder: Path) -> Path:
        y, x = np.mgrid[0:height, 0:width].astype(np.float32)
        rings = 127 + 120 * np.sin(np.hypot(x - width / 3, y - height / 2) / 6)
        bars = np.where((x // 24 + y // 24) % 2 == 0, 230.0, 30.0)
        rgb = np.stack([rings, bars, x * 255 / width], axis=-1).clip(0, 255).astype(np.uint8)
        path = folder / f"picture-{width}x{height}.png"
        Image.fromarray(rgb).save(path)
        return path

    return make


_HD = {"resolution": "1080p"}
_PHOTO = {"upscale_image.models.model": PHOTO}

CELLS: list[Cell] = [
    {"entry": "upscale_image", "name": "x4", "payload": dict(_HD), "files": {"image": picture(480, 270)},
     "check": size(1920, 1080)},
    {"entry": "upscale_image", "name": "x2-tiled", "payload": dict(_HD), "files": {"image": picture(960, 540)},
     "models": _PHOTO, "check": size(1920, 1080)},
    {"entry": "upscale_image", "name": "two-passes", "payload": dict(_HD), "files": {"image": picture(240, 135)},
     "models": _PHOTO, "check": size(1920, 1080)},
]
