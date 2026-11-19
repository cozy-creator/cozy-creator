"""Tiny Qwen3.x for the `qwen3` package: the dense hybrid stack (three gated-delta-net layers, one attention layer; Qwen3.5,
Qwen3.6-27B, Qwen3.8-27B) as its default, and the mixture of experts (Qwen3.6-35B-A3B) as a request picks it (`model`), a few
channels wide, each with its multi-token-prediction head, Qwen's tokenizer and the Qwen3-VL processor (Qwen's own 248k
vocabulary is not on this computer: the token ids differ, the code paths do not)."""

from __future__ import annotations

import shutil
from collections.abc import Callable
from pathlib import Path
from types import ModuleType

import av
import numpy as np
import torch
from cozy_runtime import lm
from PIL import Image
from safetensors.torch import load_file, save_file
from transformers import Qwen3_5Config, Qwen3_5ForConditionalGeneration, Qwen3_5MoeConfig, Qwen3_5MoeForConditionalGeneration
from transformers.models.qwen3_5 import modeling_qwen3_5
from transformers.models.qwen3_5_moe import modeling_qwen3_5_moe

from cell import NARROW, Cell, answers, module, packages, qwen_tokenizer, save, wrote

#: The package's default (dense) and the mixture of experts a cell picks.
MODEL = "llmfan46/qwen3.8-27b-uncensored-heretic-native-mtp-preserved@0.1.0"
MOE = "llmfan46/qwen3.6-35b-a3b-uncensored-heretic-native-mtp-preserved@0.1.0"


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


def write(folder: Path, model: torch.nn.Module, space: ModuleType = modeling_qwen3_5) -> Path:
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


def dense(folder: Path) -> Path:
    """The dense model, from seed 0: what a package that reads a Qwen3.5 checkpoint (pan2's reasoner) binds too."""
    torch.manual_seed(0)
    return write(folder, module(Qwen3_5ForConditionalGeneration(config())))


def moe(folder: Path) -> Path:
    """35B-A3B's layout: routed and shared experts, each layer's in Transformers' fused banks as the release stores them
    (`experts.gate_up_proj`, `experts.down_proj`), which TensorFS's import stores one expert at a time; from seed 0."""
    torch.manual_seed(0)
    plain = config().to_dict()
    text = {**plain["text_config"], "num_experts": 4, "num_experts_per_tok": 2, "moe_intermediate_size": 32,
            "shared_expert_intermediate_size": 32}
    made = Qwen3_5MoeConfig(text_config=text, vision_config=plain["vision_config"],
                            **{k: plain[k] for k in ("image_token_id", "video_token_id", "vision_start_token_id",
                                                     "vision_end_token_id", "tie_word_embeddings")})
    return write(folder, module(Qwen3_5MoeForConditionalGeneration(made)), modeling_qwen3_5_moe)


def unpaired(folder: Path, store: Path) -> None:
    """gpt-oss's banks (xs-220, #4037's review): gate-and-up [E, H, 2I] and down [E, I, H], so with I = H its down alone has the
    shape of Transformers' [E, H, I]. TensorFS's import splits a layer's banks only as a pair: both stay fused, and it says so.
    Split alone, each expert's down weight would be named transposed."""
    import json

    import tensorfs

    folder.mkdir(parents=True, exist_ok=True)
    (folder / "config.json").write_text(json.dumps({"model_type": "gpt_oss", "num_local_experts": 4, "hidden_size": 16,
                                                    "intermediate_size": 16}))
    stem = "model.layers.0.mlp.experts"
    torch.manual_seed(0)
    save_file({f"{stem}.gate_up_proj": torch.randn(4, 16, 32, dtype=torch.bfloat16),
               f"{stem}.down_proj": torch.randn(4, 16, 16, dtype=torch.bfloat16)}, folder / "model.safetensors", metadata={"format": "pt"})
    said: list[str] = []
    checkpoint = tensorfs.convert_to_cozytensors(str(folder), cache_dir=store, progress=lambda line, *_: said.append(line))
    names = {name for component in checkpoint.components.values() for name in component.tensors.keys()}
    split = sorted(name for name in names if name.startswith(f"{stem}.") and name.count(".") > stem.count(".") + 1)
    assert not split, f"an unpaired bank was split, its experts' weights named transposed: {split[:2]}"
    assert {f"{stem}.gate_up_proj", f"{stem}.down_proj"} <= names, f"the fused banks aren't kept: {sorted(names)[:4]}"
    assert any(line.startswith("warning:") and stem in line for line in said), f"no warning names the banks kept fused: {said}"


def models(root: Path, store: Path) -> dict[str, Path | str]:
    unpaired(root / "unpaired", store)
    return {MODEL: dense(root / "dense"), MOE: moe(root / "moe")}


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
    """Each request on the default, then on the mixture of experts (`moe-*`)."""
    plain: list[Cell] = [
        {"entry": "ask", "name": "text", "payload": {"prompt": "What is two plus two?", "max_tokens": 12},
         "check": answers(1)},
        {"entry": "ask", "name": "image", "payload": {"prompt": "What is in this picture?", "max_tokens": 12},
         "files": {"images.0": picture}, "check": answers(1)},
        {"entry": "ask", "name": "video", "payload": {"prompt": "What happens here?", "max_tokens": 12, "max_frames": 4},
         "files": {"videos.0": clip}, "check": answers(1)},
        {"entry": "ask", "name": "sampled", "payload": {"prompts": [{"text": "Name a color."}, {"text": "Name a fruit."}],
                                                        "max_tokens": 8, "temperature": 0.8, "seed": 3}, "check": answers(2)},
        {"entry": "caption", "name": "image", "payload": {"max_tokens": 12}, "files": {"images.0": picture},
         "check": answers(1)},
        # xs-204, Paul 11-16: "attach an LLM (ideally VLLM) to an image node, or a video node, and ask it to produce a
        # caption, and it'll produce text". The box's tool: each mention's picture or video where it's first mentioned.
        {"entry": "write", "name": "text", "payload": {"instructions": {"text": "Write one line about the sea."},
                                                      "max_tokens": 12, "seed": 3}, "check": wrote},
        {"entry": "write", "name": "mentions",
         "payload": {"instructions": {"text": "Caption <fox>, then say what moves in <clip> and in <fox>.",
                                      "references": [{"key": "fox", "type": "image"}, {"key": "clip", "type": "video"}]},
                     "max_tokens": 12, "seed": 3},
         "files": {"instructions.references.0.file": picture, "instructions.references.1.file": clip}, "check": wrote},
        {"entry": "quantize", "name": "int8", "payload": {}, "skip": NARROW},
    ]
    picked: list[Cell] = [{**cell, "name": f"moe-{cell['name']}", "models": {"model": MOE}}
                          for cell in plain if cell["entry"] != "quantize"]
    return plain + picked


CELLS = cells(picture, clip)
