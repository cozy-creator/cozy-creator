"""Tiny SeedVR2: the package's own NaDiT and video VAE (`seedvr2.model.modules`) at a few channels, with upstream's
fixed text conditioning drawn at its real shape, written as the release's three-component checkpoint (`dit`, `vae`,
`conditioning`; the `SeedVR2-7B` config) by TensorFS's writer.

SeedVR2 states the 7B geometry as constants (`DIT`, `VAE`), so the executor takes them shrunk (PATCH) and the 7B
geometry stays GPU-only. The output canvas is the real one: 1080p is the least the function restores to."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import av
import numpy as np
import tensorfs
import torch
from seedvr2 import model as seedvr

from cell import Cell, Json, quantize

#: 2 of the release's 128-wide heads and 2 blocks (256 wide, so int8.convrot's 256-column blocks fit), the 16-channel
#: latent and 33-channel input kept.
DIT: dict[str, object] = {
    **seedvr.DIT, "vid_dim": 256, "txt_dim": 256, "emb_dim": 6 * 256, "heads": 2, "head_dim": 128, "num_layers": 2,
    "block_type": ["mmdit_sr"] * 2, "window": [(4, 3, 3)] * 2,
    "window_method": ["720pwin_by_size_bysize", "720pswin_by_size_bysize"],
}
VAE: dict[str, object] = {**seedvr.VAE, "block_out_channels": (8, 8, 16, 16), "layers_per_block": 1, "norm_num_groups": 4}
PATCH: dict[str, object] = {"seedvr2.model:DIT": DIT, "seedvr2.model:VAE": VAE}
_NAMES = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32"}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    seedvr.DIT, seedvr.VAE = DIT, VAE  # this process only: the executor takes PATCH
    made = seedvr.modules()
    vae = made["vae"]
    assert isinstance(vae, torch.nn.Module)  # upstream's wrapper, typed as what the restoration calls of it
    built: dict[str, torch.nn.Module] = {"dit": made["dit"], "vae": vae, "conditioning": seedvr.Conditioning()}
    writer = tensorfs.Writer(cache_dir=store)
    generator = torch.Generator().manual_seed(1)
    for name, module in built.items():
        # The release's one construction config (its `config.json`), carried by the DiT.
        component = writer.component(name, config={"architecture": seedvr.ARCHITECTURE} if name == "dit" else None)
        for key, tensor in module.state_dict().items():
            data = tensor.detach()
            if name == "conditioning" or not bool(torch.isfinite(data).all()):
                data = (torch.randn(data.shape, generator=generator) * 0.5).to(data.dtype)
            data = data.contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=_NAMES[data.dtype],
                          shape=list(data.shape))
    return {"bytedance-seed/seedvr2-7b@0.1.0": writer.commit().id}


def clip(frames: int, width: int, height: int, sound: bool = False) -> Callable[[Path], Path]:
    def make(folder: Path) -> Path:
        path = folder / f"clip-{frames}.mp4"
        with av.open(str(path), "w") as container:
            stream = container.add_stream("libx264", rate=8)
            stream.width, stream.height, stream.pix_fmt = width, height, "yuv420p"
            audio = container.add_stream("aac", rate=16000) if sound else None
            for index in range(frames):
                y, x = np.mgrid[0:height, 0:width]
                rgb = np.stack([(x * 3 + index * 20) % 256, y * 3 % 256, np.full_like(x, 120)], axis=-1).astype(np.uint8)
                for packet in stream.encode(av.VideoFrame.from_ndarray(rgb, format="rgb24")):
                    container.mux(packet)
            for packet in stream.encode():
                container.mux(packet)
            if audio is not None:
                tone = (np.sin(np.arange(16000 * frames // 8) * 0.05) * 0.3).astype(np.float32)[None]
                chunk = av.AudioFrame.from_ndarray(tone, format="fltp", layout="mono")
                chunk.sample_rate = 16000
                for packet in audio.encode(chunk):
                    container.mux(packet)
                for packet in audio.encode():
                    container.mux(packet)
        return path

    return make


_SMALL: dict[str, Json] = {"seed": 7}

CELLS: list[Cell] = [
    {"entry": "upscale", "name": "clip", "payload": dict(_SMALL), "files": {"video": clip(5, 96, 64)}},
    {"entry": "upscale", "name": "windows", "payload": {**_SMALL, "batch_size": 5, "temporal_overlap": 1, "prepend_frames": 1,
                                                       "aspect_ratio": "16:9", "color_correction": "wavelet"},
     "files": {"video": clip(9, 96, 64, sound=True)}},
    quantize("bytedance-seed/seedvr2-7b@0.1.0", "int8", "seedvr2", "seedvr2@0.4.1"),
]
