"""Tiny MiniMax-H3-Image (se-146). Paul, 11-17: "please look into minimax h3 image as well. WE should add this model to
our roster when possible" and "keep the prompt expander as well". Without the package every cell fails; with it, text to
image, an edit of two pictures, and both through the prompt expander render (xs-199).

The package's own constructors a few channels wide, as the three checkpoints its slots default to:
- `minimaxai/minimax-h3-image`, by TensorFS's writer in the `bf16` lane's layout (what `prepare` writes and the
  int8 lanes build on): the DiT with its AdaLN as exact tables of the rows the 6 steps read (`TabledTransformer`,
  `model.table_keys`), `vae_encoder` float32, and `vae_decoder` bf16 but for the modules Diffusers keeps float32. The
  decoder paints each latent token's patch one colour (`_flat_patches`), so a picture's blocks read its latents.
- MiniMax-H3's text encoder, `minimaxai/minimax-h3`'s `text_encoder` component: the release's config (minimax-h3's
  `model-config.json`, its `cozy_h3` truncation included) at 64 wide, Qwen3-VL's processor under `processor/`.
- MiniMax-H3-Image-PE as uploaded: a Qwen3.5 VLM's Transformers folder with stand-ins for MiniMax's prompts under
  `prompt/`, in the released shape (the T2I task a `str.format` template).

The canvas is the tool's least, 4 MP. A tiny expander never closes its thought, so an upsampled cell renders the
request as written, with the tool's warning; the expander still runs, over the picture too."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

import tensorfs
import torch
from minimax_h3_image.model import conditioner, table_keys
from minimax_h3_image.transformer import build_transformer, compute_dtypes, parse_tables
from minimax_h3_image.vae import ImageDecoder, ImageEncoder, kept_fp32
from transformers import Qwen3_5ForConditionalGeneration

from cell import Cell, Json, defined, module, packages, save, size
from tiny import _shared

MODEL = "minimaxai/minimax-h3-image@0.1.1"
TEXT_ENCODER = "minimaxai/minimax-h3@1.0.0-rc.8"
EXPANDER = "minimaxai/minimax-h3-image-pe@0.1.0"
#: The text encoder's width, which the DiT's context embedder reads.
TEXT_WIDTH = 64
#: Diffusers 0.43's `MiniMaxH3ImageTransformer2DModel`: two heads of 32, 24 of them rotated (3 axes x 4 x 2), a head
#: per step.
DIT: dict[str, Json] = {
    "attention_head_dim": 32, "ffn_dim": 128, "final_norm_eps": 1e-05, "freq_dim": 32, "hidden_size": 64,
    "in_channels": 24, "norm_eps": 1e-05, "num_attention_heads": 2, "num_layers": 2, "num_pdd_heads": 6,
    "num_refiner_layers": 2, "patch_size": [1, 2, 2], "qk_norm_eps": 1e-05, "rope_freq_dim": 4, "rope_theta": 10000,
    "text_dim": TEXT_WIDTH, "time_embed_dim": 32, "time_embed_hidden_dim": 64,
}
_NORMALIZATION = json.loads((Path(__file__).with_name("assets") / "minimax_h3" / "latent-upscaler-config.json").read_text())
#: Diffusers 0.43's `AutoencoderKLMiniMaxH3Image` (H3's encoder, a 2D ViT decoder), with H3's latent normalization.
VAE: dict[str, Json] = {
    "in_channels": 3, "out_channels": 3, "latent_channels": 24, "block_out_channels": [8, 8, 8, 16, 16, 16],
    "layers_per_block": 1, "spatial_downsample_factors": [2, 2, 2, 2, 1, 1],
    "temporal_downsample_factors": [1, 2, 2, 1, 1, 1], "norm_num_groups": 4, "norm_eps": 1e-06,
    "spatial_padding_mode": "reflect", "decoder_num_layers": 1, "decoder_num_attention_heads": 2,
    "decoder_attention_head_dim": 16, "decoder_num_register_tokens": 4, "decoder_ffn_mult": 4,
    "decoder_rope_theta": 100.0, "decoder_rope_dim_ratio": 0.5, "decoder_norm_eps": 1e-05,
    "latents_mean": _NORMALIZATION["latent_normalization"]["mean"],
    "latents_std": _NORMALIZATION["latent_normalization"]["std"],
}
#: MiniMax's prompt files, stand-ins in their released shape.
PROMPTS = {
    "prompt/t2i/system_prompt.txt": "Reason first. Then answer with one JSON object: expanded_caption, aspect_ratio.\n",
    "prompt/t2i/task_prompt.txt": 'Answer as {{"expanded_caption":"CAPTION","aspect_ratio":"16:9"}}.\n\n'
                                  "input_text: {}\noutput_text:",
    "prompt/edit/task_prompt.txt": 'Answer as {"expanded_caption":"CAPTION","target_aspect_ratio":"W:H"}.\n',
}
_NAMES = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32", torch.int64: "I64"}


def _finite(root: torch.nn.Module, seed: int) -> None:
    """Every tensor its class left undefined (`defined`: NaN) drawn from `seed`."""
    generator = torch.Generator().manual_seed(seed)
    with torch.no_grad():
        for tensor in root.state_dict().values():
            if tensor.is_floating_point() and not bool(torch.isfinite(tensor).all()):
                tensor.copy_((torch.randn(tensor.shape, generator=generator) * 0.02).to(tensor.dtype))


def _write(writer: tensorfs.Writer, name: str, root: torch.nn.Module, config: dict[str, Json], seed: int,
           dtype: Callable[[str], torch.dtype] | None = None, assets: dict[str, bytes] | None = None) -> None:
    """A component's tensors as `root` holds them (or each in `dtype(key)`), a tied tensor once."""
    _finite(root, seed)
    component = writer.component(name, config=config)
    written: set[int] = set()
    for key, tensor in root.state_dict().items():
        if tensor.numel() and tensor.data_ptr() in written:
            continue
        written.add(tensor.data_ptr())
        data = tensor.detach()
        if data.is_floating_point() and dtype is not None:
            data = data.to(dtype(key))
        data = data.contiguous()
        component.put(key, memoryview(data.reshape(-1).view(torch.uint8).numpy()).cast("B"), dtype=_NAMES[data.dtype],
                      shape=list(data.shape))
    for asset, content in (assets or {}).items():
        component.asset(asset, content)


def _flat_patches(decoder: torch.nn.Module) -> None:
    """The ViT decoder's pixel projection the same for every pixel of a token's patch, per colour channel."""
    projection = decoder.get_submodule("decoder.proj_out")
    assert isinstance(projection, torch.nn.Linear) and projection.bias is not None
    with torch.no_grad():
        for parameter in (projection.weight, projection.bias):
            rows = parameter.view(3, -1, *parameter.shape[1:])  # (channel, row x column, ...)
            rows.copy_(rows[:, :1].clone().expand_as(rows))


def _table_keys() -> Json:
    document: Json = json.loads(json.dumps(table_keys()))
    return document


def _text_config() -> dict[str, Json]:
    """MiniMax-H3's `text_encoder` section as its release states it, 64 wide (minimax-h3's own tiny's shrink)."""
    source = packages() / "minimax-h3" / "minimax_h3" / "preparation" / "assets" / "model-config.json"
    section: dict[str, Json] = json.loads(source.read_text())["text_encoder"]
    text, vision = section["text_config"], section["vision_config"]
    assert isinstance(text, dict) and isinstance(vision, dict)
    section["text_config"] = {**text, "hidden_size": TEXT_WIDTH, "intermediate_size": 128, "num_attention_heads": 4,
                              "num_key_value_heads": 2, "head_dim": 16,
                              "rope_scaling": {"mrope_interleaved": True, "mrope_section": [4, 2, 2], "rope_type": "default"}}
    section["vision_config"] = {**vision, "depth": 3, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2,
                                "out_hidden_size": TEXT_WIDTH, "deepstack_visual_indexes": [0, 1, 2]}
    return section


def _expander(folder: Path) -> Path:
    """MiniMax-H3-Image-PE as uploaded: the Transformers folder, its processor files at the root, its prompts."""
    with defined():
        model = module(Qwen3_5ForConditionalGeneration(_shared.qwen3_5_vl(mtp_num_hidden_layers=0)))
    _finite(model, 5)
    save(model.to(torch.bfloat16), folder)
    _shared.qwen_processor(folder)
    for name, text in PROMPTS.items():
        (folder / name).parent.mkdir(parents=True, exist_ok=True)
        (folder / name).write_text(text)
    return folder


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    text = _text_config()
    with defined():
        dit = compute_dtypes(build_transformer(DIT, parse_tables(table_keys())))
        encoder, decoder = ImageEncoder(VAE), ImageDecoder(VAE)
        conditioning = module(conditioner(text))
    _flat_patches(decoder)
    image = tensorfs.Writer(cache_dir=store)
    _write(image, "transformer", dit, {**DIT, "adaln_tables": _table_keys()}, 1)
    _write(image, "vae_encoder", encoder, VAE, 2, lambda key: torch.float32)
    _write(image, "vae_decoder", decoder, VAE, 3, lambda key: torch.float32 if kept_fp32(key) else torch.bfloat16)
    processor = root / "h3-processor"
    _shared.qwen_processor(processor)
    h3 = tensorfs.Writer(cache_dir=store)
    _write(h3, "text_encoder", conditioning, text, 4,
           assets={f"processor/{path.name}": path.read_bytes() for path in sorted(processor.iterdir()) if path.is_file()})
    torch.manual_seed(1)
    return {MODEL: image.commit().id, TEXT_ENCODER: h3.commit().id, EXPANDER: _expander(root / "expander")}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX, _HAT = _shared.picture("fox", 640, 512), _shared.picture("hat", 384, 384)
#: 4 MP, the tool's least; the prompt as written unless the cell upsamples it. An edit's canvas is Picture 1's 5:4.
_SMALL: dict[str, Json] = {"seed": 7, "megapixels": 4, "prompt_upsampling": False}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": {"instructions": _prompt("a red fox in fresh snow"), **_SMALL},
     "check": size(2048, 2048)},
    {"entry": "generate", "name": "upsampled",
     "payload": {"instructions": _prompt("a red fox"), **_SMALL, "prompt_upsampling": True}, "check": size(2048, 2048)},
    {"entry": "generate", "name": "edit",
     "payload": {"instructions": _prompt("give <fox> the hat in <hat>", "fox", "hat"), **_SMALL},
     "files": {"instructions.references.0.file": _FOX, "instructions.references.1.file": _HAT}, "check": size(2304, 1824)},
    {"entry": "generate", "name": "edit-upsampled",
     "payload": {"instructions": _prompt("the fox in <fox> in a winter forest", "fox"), **_SMALL,
                 "prompt_upsampling": True},
     "files": {"instructions.references.0.file": _FOX}, "check": size(2304, 1824)},
]
