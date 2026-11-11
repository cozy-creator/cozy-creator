"""Tiny Qwen3.6 (35B-A3B's mixture of experts): Qwen3.5's hybrid stack with routed and shared experts, a few channels
wide, with its multi-token-prediction head, Qwen's tokenizer and the Qwen3-VL processor."""

from __future__ import annotations

from pathlib import Path

import torch
from safetensors.torch import load_file, save_file
from transformers import Qwen3_5MoeConfig, Qwen3_5MoeForConditionalGeneration
from transformers.models.qwen3_5_moe import modeling_qwen3_5_moe

from cell import Cell, module
from tiny.qwen3_5 import clip, config, picture, write
from tiny.qwen3_5 import cells as qwen_cells

REF = "qwen/qwen3.6-35b-a3b@0.1.0"


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    dense = config().to_dict()
    text = {**dense["text_config"], "num_experts": 4, "num_experts_per_tok": 2, "moe_intermediate_size": 32,
            "shared_expert_intermediate_size": 32}
    moe = Qwen3_5MoeConfig(text_config=text, vision_config=dense["vision_config"],
                           **{k: dense[k] for k in ("image_token_id", "video_token_id", "vision_start_token_id",
                                                    "vision_end_token_id", "tie_word_embeddings")})
    folder = write(root / "qwen3.6", module(Qwen3_5MoeForConditionalGeneration(moe)), modeling_qwen3_5_moe)
    split(folder / "model.safetensors")
    return {REF: folder}


def split(path: Path) -> None:
    """Each fused expert bank one expert at a time, as TensorFS's `qwen3.6-35b-a3b.experts` table lays out the release
    (that table is pinned to the release's shapes): `experts.{e}.gate_and_up_proj.weight`, `experts.{e}.down_proj.weight`."""
    tensors = load_file(path)
    out: dict[str, torch.Tensor] = {}
    for key, value in tensors.items():
        bank, _, name = key.rpartition(".experts.")
        if name in ("gate_up_proj", "down_proj"):
            row = "gate_and_up_proj" if name == "gate_up_proj" else "down_proj"
            out |= {f"{bank}.experts.{e}.{row}.weight": value[e].contiguous() for e in range(value.shape[0])}
        else:
            out[key] = value
    save_file(out, path, metadata={"format": "pt"})


CELLS: list[Cell] = qwen_cells(picture, clip)
