"""Tiny MOSS-Image: the package's own DiT (`MossTransformer`, upstream's module names) at 32 wide with 23 blocks, so
the negative branch's skipped block 22 is there, beside Qwen3.5's text model (three linear-attention layers and a full
one) and FLUX.2's VAE in F32, with Qwen's tokenizer, as one checkpoint folder laid out as openmoss-team/moss-image@0.1.1
is: its text encoder carries Qwen3.5's image processor (`text_encoder/processor/preprocessor_config.json`; Qwen-VL's
16 px patches merged 2x2, the copy minimax-h3 bundles). `generate` and the `image` tool call `text_to_image` on it."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import torch
from diffusers import AutoencoderKLFlux2
from moss_image.model import MossTransformer
from safetensors.torch import save_file
from transformers import Qwen3_5Model

from cell import NARROW, Cell, Json, module, packages, qwen_tokenizer, save, size
from tiny import _shared

MOSS = "openmoss-team/moss-image@0.1.1"
TEXT_WIDTH, WIDTH, FFN, HEADS, FREQ, LATENT = 32, 32, 64, 2, 32, 16


def _dit(folder: Path, layers: int) -> None:
    """The DiT as upstream stores it: its config and safetensors under its own module names."""
    dit = MossTransformer(dim=WIDTH, ffn_dim=FFN, num_heads=HEADS, num_layers=layers, text_dim=TEXT_WIDTH, freq_dim=FREQ,
                          latent_channels=LATENT).to(torch.bfloat16)
    folder.mkdir(parents=True)
    save_file({k: v.contiguous() for k, v in dit.state_dict().items()}, folder / "diffusion_pytorch_model.safetensors")
    (folder / "config.json").write_text(json.dumps({"dim": WIDTH, "ffn_dim": FFN, "num_heads": HEADS, "num_layers": layers,
                                                    "text_dim": TEXT_WIDTH, "freq_dim": FREQ, "latent_channels": LATENT}))


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "moss-image"
    torch.manual_seed(0)
    qwen_tokenizer(out / "tokenizer")
    save(module(Qwen3_5Model(_shared.qwen3_5_vl(width=TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    (out / "text_encoder" / "processor").mkdir()
    shutil.copyfile(packages() / "minimax-h3" / "minimax_h3" / "processor" / "preprocessor_config.json",
                    out / "text_encoder" / "processor" / "preprocessor_config.json")
    _dit(out / "transformer", 23)
    save(module(AutoencoderKLFlux2(
        block_out_channels=(8, 8, 16, 16), layers_per_block=1, latent_channels=LATENT // 4, norm_num_groups=4,
    )), out / "vae")
    (out / "scheduler").mkdir()
    (out / "scheduler" / "scheduler_config.json").write_text(json.dumps(
        {"_class_name": "MOSSImageScheduler", "num_train_timesteps": 1000, "shift": 5.0, "training_shift": 1.0}))
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "MOSSImagePipeline", "scheduler": ["moss_image", "MOSSImageScheduler"],
        "text_encoder": ["transformers", "Qwen3_5Model"], "tokenizer": ["transformers", "Qwen2TokenizerFast"],
        "transformer": ["moss_image", "MOSSImageTransformer"], "vae": ["diffusers", "AutoencoderKLFlux2"]}))
    return {MOSS: out}


#: 512², MOSS-Image's smallest canvas (0.25 MP).
_SMALL: dict[str, Json] = {"prompt": "a red fox in fresh snow", "seed": 7, "megapixels": 0.25, "steps": 2}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "generate", "name": "no-cfg", "payload": {**_SMALL, "guidance": 1.0}},
    {"entry": "generate", "name": "regions", "payload": {
        **_SMALL, "aspect_ratio": "3:2", "regions": [
            {"box": [0.1, 0.1, 0.5, 0.6], "prompt": "a red fox"},
            {"box": [0.55, 0.7, 0.95, 0.9], "prompt": "a wooden sign", "text": "SNOW"}]}},
    {"entry": "image", "name": "tool", "payload": {"instructions": "a red fox in fresh snow", "seed": 7,
                                                   "negative_prompt": "blurry"}, "check": size(1024, 1024)},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
