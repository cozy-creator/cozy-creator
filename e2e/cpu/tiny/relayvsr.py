"""RelayVSR's own Wan and Flash constructors at two blocks and one attention head.

Only release geometry shrinks: Wan keeps its 128-channel head and 48 latent channels, Flash its
64-channel head, and the package runs its real keyframe ring, eager attentions and video stream.
The output remains the public 1080p canvas. CUDA/Triton kernels remain a GPU qualification.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import av
import numpy as np
import tensorfs
import torch
from relayvsr import flash, wan
from relayvsr.model import ARCHITECTURE

from cell import Cell, Json, defined

_WAN = (128, 1, 2, 256, 32, 8, (16, 32))
_FLASH = (64, 2, 1, 1)
PATCH: dict[str, object] = {
    "relayvsr.wan:WanVSR.__init__.__defaults__": _WAN,
    "relayvsr.flash:FlashVSR.__init__.__defaults__": _FLASH,
    "relayvsr.flash:ENCODER": (8, 8, 8),
    "relayvsr.flash:HEAD_WIDTH": 64,
}
_DTYPES = {torch.bfloat16: "BF16", torch.float32: "F32"}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    flash.ENCODER, flash.HEAD_WIDTH = (8, 8, 8), 64  # this process; executors take PATCH
    with defined():
        built: dict[str, torch.nn.Module] = {
            "wan": wan.WanVSR(dim=128, heads=1, layers=2, ffn=256, text=32, prompt=8, lq=(16, 32)).to(torch.bfloat16),
            "flash": flash.FlashVSR(dim=64, depth=2, heads=1, kv_heads=1),
        }
    writer = tensorfs.Writer(cache_dir=store)
    generator = torch.Generator().manual_seed(1)
    for name, module in built.items():
        component = writer.component(name, config={"architecture": ARCHITECTURE} if name == "wan" else None)
        for key, tensor in module.state_dict().items():
            data = tensor.detach()
            if not bool(torch.isfinite(data).all()):
                data = (torch.randn(data.shape, generator=generator) * 0.02).to(data.dtype)
            data = data.contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"),
                          dtype=_DTYPES[data.dtype], shape=list(data.shape))
    return {"kopper/relayvsr@0.1.1": writer.commit().id}


def clip(frames: int, sound: bool = False) -> Callable[[Path], Path]:
    def make(folder: Path) -> Path:
        path = folder / "clip.mp4"
        with av.open(str(path), "w") as container:
            stream = container.add_stream("libx264", rate=8)
            stream.width, stream.height, stream.pix_fmt = 16, 16, "yuv420p"
            audio = container.add_stream("aac", rate=16000) if sound else None
            for index in range(frames):
                y, x = np.mgrid[0:16, 0:16]
                rgb = np.stack([(x * 15 + index * 20) % 256, y * 15, np.full_like(x, 120)], axis=-1).astype(np.uint8)
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


def video(frames: int, width: int, sound: bool = False) -> Callable[[Json, list[Path]], str | None]:
    def check(result: Json, files: list[Path]) -> str | None:
        if not isinstance(result, dict) or result.get("frame_count") != frames or result.get("fps") != 8.0:
            return "the result changed the input frame count or presentation rate"
        if len(files) != 1:
            return "the function did not return one video"
        with av.open(str(files[0])) as container:
            if bool(container.streams.audio) != sound:
                return "the output changed the presence of its soundtrack"
            decoded = list(container.decode(video=0))
            if len(decoded) != frames or any((frame.width, frame.height) != (width, 1080) for frame in decoded):
                return "the output changed its frame count or 1080p canvas"
        return None

    return check


CELLS: list[Cell] = [
    {"entry": "upscale", "name": "clip", "payload": {"seed": 7, "window_size": 3},
     "files": {"video": clip(5)}, "check": video(5, 1080)},
    {"entry": "upscale", "name": "windows-audio", "payload": {"seed": 7, "window_size": 3, "aspect_ratio": "16:9"},
     "files": {"video": clip(7, sound=True)}, "check": video(7, 1920, sound=True)},
]
