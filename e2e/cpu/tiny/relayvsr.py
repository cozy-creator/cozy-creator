"""RelayVSR's own Wan and Flash constructors at two blocks and one attention head, built from configs laid out as
kopper/relayvsr@0.2.0's are (xs-183): a `model` component naming the architecture, and `wan` and `flash` each holding
upstream's own `config` (its file's `__metadata__`: crates/tensorfs/vectors/relayvsr-headers), only release geometry
shrunk.

Wan keeps its 128-channel head and 48 latent channels, Flash its 64-channel head, and the package runs its real keyframe
ring, eager attentions and video stream. The output remains the public 1080p canvas. CUDA/Triton kernels remain a GPU
qualification.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import av
import msgspec
import numpy as np
import tensorfs
import torch
from relayvsr import flash, wan
from relayvsr.flash import FlashConfig
from relayvsr.model import ARCHITECTURE
from relayvsr.wan import WanConfig

from cell import Cell, Json, defined

#: `wan.safetensors`' config (`refvsr-wan-v1`) at one 128-channel head, 2 layers, a 256-wide FFN, 32 text channels and
#: a 16/32 LQ projection.
_WAN: dict[str, Json] = {
    "patch_size": [1, 2, 2], "num_attention_heads": 1, "attention_head_dim": 128, "in_channels": 48, "out_channels": 48,
    "text_dim": 32, "freq_dim": 256, "ffn_dim": 256, "num_layers": 2, "cross_attn_norm": True,
    "qk_norm": "rms_norm_across_heads", "eps": 1e-06, "image_dim": None, "rope_max_seq_len": 1024,
    "pos_embed_seq_len": None, "lq_proj_hidden_dim1": 16, "lq_proj_hidden_dim2": 32, "use_block_causal_attention": True,
    "num_frames_per_block": 1, "local_attn_size": -1, "sink_size": 0, "flex_attention_block_size": 128,
    "spatial_window_size": [22, 40], "spatial_attention_backend": "triton", "spatial_attention_mode": "local",
    "temporal_context_frames": 4, "temporal_rope": {"policy": "fixed_end", "training_frames": 8},
}
#: `flash.safetensors`' config (`refvsr-flash-v1`, variant S) with an 8-channel frame encoder and a 64-wide head.
_FLASH: dict[str, Json] = {
    "condition_channels": 48, "encoder_channels": [8, 8, 8], "gradient_checkpointing": "none",
    "mlp_ratio": 2.6666666666666665, "norm_eps": 1e-05, "rope_theta": 10000.0,
    "spatial_attention": {"backend": "auto", "boundary": "truncated", "condition_self_window_size": [48, 48],
                          "end_condition_window_size": [48, 48], "interval": "half_open",
                          "start_condition_window_size": [48, 48], "type": "query_centered",
                          "video_window_size": [24, 24]},
    "spatial_head_dim": 64, "temporal_rope_policy": "current_relative", "training_execution": "streaming",
    "upscale_factor": 4, "variant": "S", "window_size": 2, "condition_mode": "endpoints",
}
#: The release geometries the package states as constants: Flash's variant S (depth, width, heads, K/V heads) at two
#: blocks, 64 wide, one head; Wan's prompt at 8 tokens, not 512.
_VARIANTS = {**flash.VARIANTS, "S": (2, 64, 1, 1)}
_PROMPT = 8
PATCH: dict[str, object] = {"relayvsr.flash:VARIANTS": _VARIANTS, "relayvsr.wan:PROMPT": _PROMPT}
_DTYPES = {torch.bfloat16: "BF16", torch.float32: "F32"}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    flash.VARIANTS, wan.PROMPT = _VARIANTS, _PROMPT  # this process; executors take PATCH
    configs = {"wan": _WAN, "flash": _FLASH}
    with defined():
        built: dict[str, torch.nn.Module] = {
            "wan": wan.WanVSR(msgspec.convert(_WAN, WanConfig)).to(torch.bfloat16),
            "flash": flash.FlashVSR(msgspec.convert(_FLASH, FlashConfig)),
        }
    writer = tensorfs.Writer(cache_dir=store)
    writer.component("model", config={"architecture": ARCHITECTURE})
    generator = torch.Generator().manual_seed(1)
    for name, module in built.items():
        component = writer.component(name, config=configs[name])
        for key, tensor in module.state_dict().items():
            data = tensor.detach()
            if not bool(torch.isfinite(data).all()):
                data = (torch.randn(data.shape, generator=generator) * 0.02).to(data.dtype)
            data = data.contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"),
                          dtype=_DTYPES[data.dtype], shape=list(data.shape))
    return {"kopper/relayvsr@0.2.0": writer.commit().id}


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
