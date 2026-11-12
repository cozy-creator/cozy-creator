"""Tiny P-H3-Image: the package's own constructors (MiniMax H3's DiT as uploaded, `bf16-full`; Pruna's text connector;
the 24-channel VAE with its one-frame ViT decoder) and Qwen3-VL (36 language layers: the connector reads 12 ... 35) and a
Qwen3.5 upsampler a few channels wide, each with Qwen's tokenizer and Qwen3-VL's processor, written as the seven
components of `prunaai/p-h3-image` by TensorFS's own writer. The VAE keeps H3's learned latent normalization, and its
decoder paints each latent token's 16x16 patch one colour (`_flat_patches`): a random pixel projection averages within a
patch to nearly its bias, so a picture's blocks (drive.signature) would not read its latents.

The canvas is the card's smallest, 1 MP (the 1-16 MP ladder is the tool's): a tiny model runs it on the CPU."""

from __future__ import annotations

import json
from pathlib import Path

import tensorfs
import torch
from p_h3_image.model import TextConnector
from p_h3_image.transformer import build_transformer, compute_dtypes
from p_h3_image.vae import ImageVAE
from transformers import PreTrainedConfig, Qwen3_5ForConditionalGeneration, Qwen3VLForConditionalGeneration

from cell import NARROW, Cell, Json, defined, module, size
from tiny import _shared

MODEL = "prunaai/p-h3-image@0.2.0"
#: Qwen3-VL's text width, and the DiT's text channels the connector projects it to.
TEXT_WIDTH, TEXT_DIM = 32, 64
#: The DiT as uploaded (computed modulation): two heads of 32, 24 of them rotated (3 axes x 4 x 2).
DIT: dict[str, Json] = {
    "attention_head_dim": 32, "audio_in_channels": 32, "ffn_dim": 128, "final_norm_eps": 1e-05, "freq_dim": 32,
    "hidden_size": 64, "in_channels": 24, "norm_eps": 1e-05, "num_attention_heads": 2, "num_layers": 2,
    "num_refiner_layers": 2, "patch_size": [1, 2, 2], "qk_norm_eps": 1e-05, "rope_freq_dim": 4, "rope_theta": 10000,
    "text_dim": TEXT_DIM, "time_embed_dim": 32, "time_embed_hidden_dim": 64,
}
_NORMALIZATION = json.loads((Path(__file__).with_name("assets") / "minimax_h3" / "latent-upscaler-config.json").read_text())
VAE: dict[str, Json] = {
    "block_out_channels": [8, 8, 8, 16, 16, 16], "layers_per_block": 1, "norm_num_groups": 4, "decoder_num_layers": 1,
    "decoder_num_attention_heads": 2, "decoder_attention_head_dim": 16,
    "latents_mean": _NORMALIZATION["latent_normalization"]["mean"], "latents_std": _NORMALIZATION["latent_normalization"]["std"],
}
_NAMES = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32", torch.int64: "I64"}


def _write(writer: tensorfs.Writer, name: str, root: torch.nn.Module, config: dict[str, Json], seed: int,
           dtype: torch.dtype | None = None, skip: tuple[str, ...] = ()) -> None:
    """A component's tensors as `root` holds them (or in `dtype`), each one its class left undefined (`defined`) drawn
    from `seed`; a tied tensor once, at its first name, as Transformers saves it."""
    generator = torch.Generator().manual_seed(seed)
    component = writer.component(name, config=config)
    written: set[int] = set()
    for key, tensor in root.state_dict().items():
        if key.startswith(skip) or (tensor.numel() and tensor.data_ptr() in written):
            continue
        written.add(tensor.data_ptr())
        data = tensor.detach()
        if data.is_floating_point():
            if not bool(torch.isfinite(data).all()):
                data = torch.randn(data.shape, generator=generator) * 0.02
            data = data.to(dtype or tensor.dtype)
        data = data.contiguous()
        component.put(key, memoryview(data.reshape(-1).view(torch.uint8).numpy()).cast("B"), dtype=_NAMES[data.dtype],
                      shape=list(data.shape))


def _processor(writer: tensorfs.Writer, name: str, folder: Path) -> None:
    """A processor folder as TensorFS imports one: its `preprocessor_config.json` the component's config, every other
    file an asset."""
    _shared.qwen_processor(folder)
    config = json.loads((folder / "preprocessor_config.json").read_text())
    component = writer.component(name, config=config, config_file="preprocessor_config.json")
    for path in sorted(folder.iterdir()):
        if path.is_file() and path.name != "preprocessor_config.json":
            component.asset(path.name, path.read_bytes())


def _flat_patches(vae: torch.nn.Module) -> None:
    """The ViT decoder's pixel projection the same for every pixel of a token's patch, per colour channel."""
    projection = vae.get_submodule("decoder.proj_out")
    assert isinstance(projection, torch.nn.Linear) and projection.bias is not None
    with torch.no_grad():
        for parameter in (projection.weight, projection.bias):
            rows = parameter.view(3, -1, *parameter.shape[1:])  # (channel, frame x row x column, ...)
            rows.copy_(rows[:, :1].clone().expand_as(rows))


def _config(value: PreTrainedConfig) -> dict[str, Json]:
    """A Transformers config as its release's config.json holds it."""
    document: dict[str, Json] = json.loads(value.to_json_string())
    return document


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    writer = tensorfs.Writer(cache_dir=store)
    # Qwen3-VL-4B ties its head to the embeddings: the package replaces it, and the release stores none.
    text, upsampler = _shared.qwen3_vl(36, TEXT_WIDTH, tie_word_embeddings=True), _shared.qwen3_5_vl()
    with defined():
        dit = compute_dtypes(build_transformer(DIT))
        vae = module(ImageVAE.build(VAE))
        connector = TextConnector(text_hidden_size=TEXT_WIDTH, out_channels=TEXT_DIM)
        encoder = module(Qwen3VLForConditionalGeneration(text))
        upsampling = module(Qwen3_5ForConditionalGeneration(upsampler))
    _flat_patches(vae)
    _write(writer, "transformer", dit, DIT, 1)
    _write(writer, "vae", vae, VAE, 2, torch.float32)
    _write(writer, "text_connector", connector, {"text_hidden_size": TEXT_WIDTH, "out_channels": TEXT_DIM}, 3,
           torch.float32)
    _write(writer, "text_encoder", encoder, _config(text), 4, torch.bfloat16, ("lm_head.",))
    _processor(writer, "processor", root / "processor")
    _write(writer, "prompt_upsampler", upsampling, _config(upsampler), 5, torch.bfloat16)
    _processor(writer, "prompt_upsampler_processor", root / "prompt_upsampler_processor")
    return {MODEL: writer.commit().id}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 640, 512)
#: 1 MP, the card's smallest; the prompt as written unless the cell upsamples it.
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 1, "prompt_upsampling": False}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"instructions": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(1024, 1024)},
    {"entry": "generate", "name": "upsampled",
     "payload": {"instructions": _prompt("a red fox"), **_SMALL, "prompt_upsampling": True}},
    {"entry": "generate", "name": "edit", "payload": {"instructions": _prompt("give <fox> a hat", "fox"), **_SMALL},
     "files": {"instructions.references.0.file": _FOX}},
    {"entry": "upscale", "name": "1mp", "payload": {"megapixels": 1, "seed": 7}, "files": {"image": _FOX}},
    {"entry": "pruned", "name": "bf16", "payload": {},
     "skip": "the `bf16` lane's AdaLN tables are written on CUDA only (adaln_tables_need_cuda), at the release's "
             "modulation shape (5376 wide, 50 blocks)"},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]
