"""Tiny Qwen3.5: its hybrid stack (three gated-delta-net layers, one attention layer) and vision tower a few channels
wide, as Qwen ships it, with Qwen's tokenizer and Qwen3-VL processor (Qwen3.5's own 248k vocabulary is not on this
computer: the token ids differ, the code paths do not)."""

from __future__ import annotations

import shutil
from collections.abc import Callable
from pathlib import Path

import av
import numpy as np
import torch
from cozy_runtime import lm
from PIL import Image
from safetensors.torch import load_file, save_file
from transformers import Qwen3_5Config, Qwen3_5ForConditionalGeneration
from transformers.models.qwen3_5 import modeling_qwen3_5

from cell import Cell, answers, module, packages, qwen_tokenizer, save

REF = "qwen/qwen3.5-9b@0.2.0"


def config() -> Qwen3_5Config:
    return Qwen3_5Config(
        text_config={"vocab_size": 151936, "hidden_size": 64, "mtp_num_hidden_layers": 1, "intermediate_size": 128, "num_hidden_layers": 4,
                     "num_attention_heads": 2, "num_key_value_heads": 1, "head_dim": 32, "linear_key_head_dim": 16,
                     "linear_value_head_dim": 16, "linear_num_key_heads": 2, "linear_num_value_heads": 4,
                     "layer_types": ["linear_attention"] * 3 + ["full_attention"], "tie_word_embeddings": False,
                     "rope_parameters": {"rope_type": "default", "rope_theta": 10000000.0, "partial_rotary_factor": 0.25,
                                         "mrope_interleaved": True, "mrope_section": [2, 1, 1]}},
        vision_config={"depth": 1, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2, "out_hidden_size": 64,
                       "patch_size": 16, "spatial_merge_size": 2, "temporal_patch_size": 2},
        image_token_id=151655, video_token_id=151656, vision_start_token_id=151652, vision_end_token_id=151653,
        tie_word_embeddings=False,
    )


def write(folder: Path, model: torch.nn.Module, space: object = modeling_qwen3_5) -> Path:
    """The checkpoint as Qwen's release lays it out: the model with its multi-token-prediction head (`mtp.*`, which
    Transformers does not build; the Runtime drafts with it), its tokenizer and its processors' settings."""
    save(model.to(torch.bfloat16), folder)
    head = module(lm.Draft(getattr(model, "config"), space))
    weights = folder / "model.safetensors"
    save_file(load_file(weights) | {k: v.to(torch.bfloat16).contiguous() for k, v in head.state_dict().items()}, weights,
              metadata={"format": "pt"})
    qwen_tokenizer(folder)
    processor = packages() / "minimax-h3" / "minimax_h3" / "processor"
    for name in ("preprocessor_config.json", "video_preprocessor_config.json"):
        shutil.copyfile(processor / name, folder / name)
    return folder


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    return {REF: write(root / "qwen3.5", module(Qwen3_5ForConditionalGeneration(config())))}


def picture(folder: Path) -> Path:
    y, x = np.mgrid[0:96, 0:128].astype(np.float32)
    path = folder / "fox.png"
    Image.fromarray(np.stack([x * 2, y * 2, (x + y) % 255], axis=-1).astype(np.uint8)).save(path)
    return path


def clip(folder: Path) -> Path:
    """A second of 64x64 video at 8 fps, each frame a shifted gradient."""
    path = folder / "clip.mp4"
    with av.open(str(path), "w") as container:
        stream = container.add_stream("libx264", rate=8)
        stream.width, stream.height, stream.pix_fmt = 64, 64, "yuv420p"
        for index in range(8):
            y, x = np.mgrid[0:64, 0:64]
            rgb = np.stack([(x * 4 + index * 16) % 256, y * 4, np.full_like(x, 128)], axis=-1).astype(np.uint8)
            for packet in stream.encode(av.VideoFrame.from_ndarray(rgb, format="rgb24")):
                container.mux(packet)
        for packet in stream.encode():
            container.mux(packet)
    return path


def cells(picture: Callable[[Path], Path], clip: Callable[[Path], Path]) -> list[Cell]:
    return [
        {"entry": "ask", "name": "text", "payload": {"prompt": "What is two plus two?", "max_tokens": 12},
         "check": answers(1)},
        {"entry": "ask", "name": "image", "payload": {"prompt": "What is in this picture?", "max_tokens": 12},
         "files": {"images.0": picture}, "check": answers(1)},
        {"entry": "ask", "name": "video", "payload": {"prompt": "What happens here?", "max_tokens": 12, "max_frames": 4},
         "files": {"videos.0": clip}, "check": answers(1)},
        {"entry": "ask", "name": "sampled", "payload": {"prompts": [{"prompt": "Name a color."}, {"prompt": "Name a fruit."}],
                                                        "max_tokens": 8, "temperature": 0.8, "seed": 3}, "check": answers(2)},
        {"entry": "caption", "name": "image", "payload": {"max_tokens": 12}, "files": {"images.0": picture},
         "check": answers(1)},
        {"entry": "quantize", "name": "int8", "payload": {}, "skip": "weights entry: the driver does not grant Invoke.sources yet"},
    ]


CELLS = cells(picture, clip)
