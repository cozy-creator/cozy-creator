"""Tiny MOSS-Image: the package's own DiT (`MossTransformer`, upstream's module names) at 32 wide with 23 blocks, so
the negative branch's skipped block 22 is there, beside Qwen3.5's text model (three linear-attention layers and a full
one) and FLUX.2's VAE in F32, with Qwen's tokenizer, as one checkpoint folder laid out as openmoss-team/moss-image@0.1.1
is: its text encoder carries Qwen3.5's image processor (`text_encoder/processor/preprocessor_config.json`; Qwen-VL's
16 px patches merged 2x2, the copy minimax-h3 bundles). `generate` and the `image` tool call `text_to_image` on it.

Moss-Image-Edit and thedeoxen's pose control LoRA are laid out as TensorFS's `moss.lora` imports them: one `transformer`
component of PEFT factors under the DiT's names (every block's six linears), `adapter_config.json` stating `r` and
`lora_alpha`. `generate` stacks one on `model` for its `edit` or `control` call, as a caller LoRA (drive.py `compose`)."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import tensorfs
import torch
from diffusers import AutoencoderKLFlux2
from moss_image.model import MossTransformer
from safetensors.torch import save_file
from transformers import Qwen3_5Model

from cell import NARROW, STEERS, Cell, Json, module, packages, qwen_tokenizer, save, size
from tiny import _shared

MOSS = "openmoss-team/moss-image@0.1.1"
EDIT, POSE = "chinoll/moss-image-edit@0.1.0", "thedeoxen/moss-image-pose-controlnet@0.1.0"
TEXT_WIDTH, WIDTH, FFN, HEADS, FREQ, LATENT, LAYERS = 32, 32, 64, 2, 32, 16, 23
#: Each block's linears an adapter adapts, (in, out).
_SITES = {"q": (WIDTH, WIDTH), "k": (WIDTH, WIDTH), "v": (WIDTH, WIDTH), "o": (WIDTH, WIDTH),
          "ffn_up_gate": (WIDTH, 2 * FFN), "ffn_down": (FFN, WIDTH)}


def _dit(folder: Path, layers: int) -> None:
    """The DiT as upstream stores it: its config and safetensors under its own module names."""
    dit = MossTransformer(dim=WIDTH, ffn_dim=FFN, num_heads=HEADS, num_layers=layers, text_dim=TEXT_WIDTH, freq_dim=FREQ,
                          latent_channels=LATENT).to(torch.bfloat16)
    folder.mkdir(parents=True)
    save_file({k: v.contiguous() for k, v in dit.state_dict().items()}, folder / "diffusion_pytorch_model.safetensors")
    (folder / "config.json").write_text(json.dumps({"dim": WIDTH, "ffn_dim": FFN, "num_heads": HEADS, "num_layers": layers,
                                                    "text_dim": TEXT_WIDTH, "freq_dim": FREQ, "latent_channels": LATENT}))


def _lora(store: Path, seed: int, rank: int) -> str:
    """An adapter at `rank` (each states `lora_alpha` = `r`) on every block's linears, bf16."""
    sites = [f"layers.{layer}.proj.{site}" for layer in range(LAYERS) for site in _SITES]
    writer = tensorfs.Writer(cache_dir=store)
    targets: list[Json] = list(_SITES)
    component = writer.component("transformer", cls=None, config_file="adapter_config.json",
                                 config={"lora_alpha": rank, "peft_type": "LORA", "r": rank, "target_modules": targets})
    generator = torch.Generator().manual_seed(seed)
    for site in sites:
        inputs, outputs = _SITES[site.rsplit(".", 1)[1]]
        for factor, shape in (("lora_A", [rank, inputs]), ("lora_B", [outputs, rank])):
            data = torch.randn(shape, generator=generator).to(torch.bfloat16).contiguous()
            component.put(f"{site}.{factor}.weight", memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16",
                          shape=shape)
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "moss-image"
    torch.manual_seed(0)
    qwen_tokenizer(out / "tokenizer")
    save(module(Qwen3_5Model(_shared.qwen3_5_vl(width=TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    (out / "text_encoder" / "processor").mkdir()
    shutil.copyfile(packages() / "minimax-h3" / "minimax_h3" / "processor" / "preprocessor_config.json",
                    out / "text_encoder" / "processor" / "preprocessor_config.json")
    _dit(out / "transformer", LAYERS)
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
    return {MOSS: out, EDIT: _lora(store, 1, 4), POSE: _lora(store, 2, 4)}


#: 512², MOSS-Image's smallest canvas (0.25 MP).
_SMALL: dict[str, Json] = {"prompt": {"text": "a red fox in fresh snow"}, "seed": 7, "megapixels": 0.25, "steps": 2}
#: generate/text's request mentioning a picture, its words read the same ("fox").
_EDIT: dict[str, Json] = {**_SMALL, "prompt": {"text": "a red <fox> in fresh snow", "references": [{"key": "fox", "type": "image"}]}}
_POSED: dict[str, Json] = {**_SMALL, "control": [{"kind": "pose"}]}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "generate", "name": "no-cfg", "payload": {**_SMALL, "guidance": 1.0}},
    {"entry": "generate", "name": "regions", "payload": {
        **_SMALL, "aspect_ratio": "3:2", "regions": [
            {"box": [0.1, 0.1, 0.5, 0.6], "prompt": "a red fox"},
            {"box": [0.55, 0.7, 0.95, 0.9], "prompt": "a wooden sign", "text": "SNOW"}]}},
    {"entry": "image", "name": "tool", "payload": {"instructions": {"text": "a red fox in fresh snow"}, "seed": 7,
                                                   "negative_prompt": "blurry"}, "check": size(1024, 1024)},
    # `edit` and `control` as `generate` calls them, without the LoRA it stacks: what an adapter not applied makes.
    {"entry": "edit", "name": "plain", "payload": _EDIT, "files": {"prompt.references.0.file": _shared.picture("fox", 512, 512)},
     "check": size(512, 512)},
    {"entry": "control", "name": "plain", "payload": _POSED, "files": {"control.0.image": _shared.bones("bones", 512, 512)},
     "check": size(512, 512)},
    # With a picture, Moss-Image-Edit on `model`: away from its plain twin, and another picture steers it again.
    {"entry": "generate", "name": "edit", "payload": _EDIT, "files": {"prompt.references.0.file": _shared.picture("fox", 512, 512)},
     "check": size(512, 512), "apart": {"edit/plain": STEERS}},
    {"entry": "generate", "name": "edit-other", "payload": _EDIT,
     "files": {"prompt.references.0.file": _shared.lines("drawn", 512, 512)}, "check": size(512, 512),
     "apart": {"generate/edit": STEERS}},
    # A pose map, thedeoxen's pose LoRA on `model`: away from its plain twin, and the skeleton moved steers it again.
    {"entry": "generate", "name": "control", "payload": _POSED, "files": {"control.0.image": _shared.bones("bones", 512, 512)},
     "check": size(512, 512), "apart": {"control/plain": STEERS}},
    {"entry": "generate", "name": "control-moved", "payload": _POSED,
     "files": {"control.0.image": _shared.bones("moved", 512, 512, dx=-0.2)}, "check": size(512, 512),
     "apart": {"generate/control": STEERS}},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
