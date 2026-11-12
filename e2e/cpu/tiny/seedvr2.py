"""Tiny SeedVR2: the package's own NaDiT and video VAE at a few channels, with upstream's fixed text conditioning drawn at
its real shape, laid out as bytedance-seed/seedvr2-7b@0.1.1 is (xs-183) by TensorFS's writer: `dit`, `vae`,
`conditioning`, and a `model` component naming the architecture and carrying upstream's two configs, which the package
reads (`seedvr2.model.settings`). The configs are upstream's (ByteDance-Seed/SeedVR e4de8c24) with only the geometry
shrunk: 2 of the 7B's 128-wide heads in 2 blocks (256 wide, so int8.convrot's 256-column blocks fit), the 16-channel
latent and 33-channel input kept; the VAE at 8-16 channels. The output canvas is the real one: 1080p is the least the
function restores to."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import cast

import av
import numpy as np
import tensorfs
import torch
from cozy_runtime.author import ModelAssets
from seedvr2 import model as seedvr

from cell import Cell, Json, defined, quantize

#: configs_7b/main.yaml: vid_dim 3072, heads 24 and num_layers 36 made 256, 2 and 2.
MAIN = """\
__object__:
  path: projects.video_diffusion_sr.train
  name: VideoDiffusionTrainer

dit:
  model:
    __object__:
      path: models.dit.nadit
      name: NaDiT
      args: as_params
    vid_in_channels: 33
    vid_out_channels: 16
    vid_dim: 256
    txt_in_dim: 5120
    txt_dim: ${.vid_dim}
    emb_dim: ${eval:'6 * ${.vid_dim}'}
    heads: 2
    head_dim: 128 # llm-like
    expand_ratio: 4
    norm: fusedrms
    norm_eps: 1e-5
    ada: single
    qk_bias: False
    qk_rope: True
    qk_norm: fusedrms
    patch_size: [ 1,2,2 ]
    num_layers: 2 # llm-like
    shared_mlp: False
    shared_qkv: False
    mlp_type: normal
    block_type: ${eval:'${.num_layers} * ["mmdit_sr"]'} # space-full
    window: ${eval:'${.num_layers} * [(4,3,3)]'} # space-full
    window_method: ${eval:'${.num_layers} // 2 * ["720pwin_by_size_bysize","720pswin_by_size_bysize"]'} # space-full
  compile: False
  gradient_checkpoint: True
  fsdp:
    sharding_strategy: _HYBRID_SHARD_ZERO2

ema:
  decay: 0.9998

vae:
  model:
    __inherit__: models/video_vae_v3/s8_c16_t4_inflation_sd3.yaml
    freeze_encoder: False
    # gradient_checkpoint: True
  slicing:
    split_size: 4
    memory_device: same
  memory_limit:
    conv_max_mem: 0.5
    norm_max_mem: 0.5
  checkpoint: ./ckpts/ema_vae.pth
  scaling_factor: 0.9152
  compile: False
  grouping: False
  dtype: bfloat16

diffusion:
  schedule:
    type: lerp
    T: 1000.0
  sampler:
    type: euler
    prediction_type: v_lerp
  timesteps:
    training:
      type: logitnormal
      loc: 0.0
      scale: 1.0
    sampling:
      type: uniform_trailing
      steps: 50
    transform: True
  loss:
    type: v_lerp
  cfg:
    scale: 7.5
    rescale: 0

condition:
  i2v: 0.0
  v2v: 0.0
  sr: 1.0
  noise_scale: 0.25
"""
#: models/video_vae_v3/s8_c16_t4_inflation_sd3.yaml: block_out_channels 128-512, layers_per_block 2 and
#: norm_num_groups 32 made 8-16, 1 and 4.
VAE = """\
__object__:
  path: models.video_vae_v3.modules.attn_video_vae
  name: VideoAutoencoderKLWrapper
  args: as_params

act_fn: silu
block_out_channels:
  - 8
  - 8
  - 16
  - 16
down_block_types:
  - DownEncoderBlock3D
  - DownEncoderBlock3D
  - DownEncoderBlock3D
  - DownEncoderBlock3D
in_channels: 3
latent_channels: 16
layers_per_block: 1
norm_num_groups: 4
out_channels: 3
slicing_sample_min_size: 4
temporal_scale_num: 2
inflation_mode: pad
up_block_types:
  - UpDecoderBlock3D
  - UpDecoderBlock3D
  - UpDecoderBlock3D
  - UpDecoderBlock3D
spatial_downsample_factor: 8
temporal_downsample_factor: 4
use_quant_conv: False
use_post_quant_conv: False
"""
_NAMES = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32"}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    configs = {seedvr.MAIN: MAIN.encode(), seedvr.VAE_CONFIG: VAE.encode()}
    with defined():
        made = seedvr.modules(seedvr.settings(ModelAssets(configs, lambda _: None)))
        # cast: upstream's VAE wrapper is an nn.Module, which the package types by the calls it makes (VideoVae)
        vae = cast(torch.nn.Module, made["vae"])
        built: dict[str, torch.nn.Module] = {"dit": made["dit"], "vae": vae, "conditioning": seedvr.Conditioning()}
    writer = tensorfs.Writer(cache_dir=store)
    generator = torch.Generator().manual_seed(1)
    for name, module in built.items():
        component = writer.component(name)
        for key, tensor in module.state_dict().items():
            data = tensor.detach()
            if name == "conditioning" or not bool(torch.isfinite(data).all()):
                data = (torch.randn(data.shape, generator=generator) * 0.5).to(data.dtype)
            data = data.contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=_NAMES[data.dtype],
                          shape=list(data.shape))
    held = writer.component("model", config={"architecture": seedvr.ARCHITECTURE})
    for path, data in configs.items():
        held.asset(path, data)
    return {"bytedance-seed/seedvr2-7b@0.1.1": writer.commit().id}


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
    quantize("bytedance-seed/seedvr2-7b@0.1.1", "int8", "seedvr2", "seedvr2@0.4.1"),
]
