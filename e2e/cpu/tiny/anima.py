"""Tiny Anima: the package's own release configs (`anima/configs`) shrunk to a few channels: Cosmos DiT, Qwen3 text
encoder, Anima's text conditioner and the Qwen-Image VAE, as the release's modular Diffusers folder in bfloat16.

Its adapters (se-127) are laid out as their 0.1.0 releases are, each its author's file uploaded as it is, at the tiny
DiT's geometry (which the executor takes: PATCH): google/siglip2-base-patch16-512 as its repo holds it (upstream's
`config.json` with the towers shrunk, SiglipModel's f32 tensors, `preprocessor_config.json`; the Gemma tokenizer, which
nothing reads, left out), and LuciferTC's IP-Adapter and kohya's four ControlNet-LLLite files as one `model` component
of their tensors, configured by their files' `__metadata__` (the header's `safetensors_metadata`)."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import numpy as np
import tensorfs
import torch
from diffusers import AnimaTextConditioner, AutoencoderKLQwenImage, CosmosTransformer3DModel
from PIL import Image
from safetensors.torch import save_file
from transformers import Qwen3Config, Qwen3Model, SiglipConfig, SiglipModel

from cell import Cell, Json, module, packages, quantize, save, size
from tiny import _shared

WIDTH = 64
#: The tiny DiT: 2 blocks of 2 heads of 16 (Anima Base: 28 of 16 of 128, `anima.adapters`).
BLOCKS, HEADS, HEAD = 2, 2, 16
DIT = HEADS * HEAD
PATCH: dict[str, object] = {"anima.adapters:BLOCKS": BLOCKS, "anima.adapters:WIDTH": DIT, "anima.adapters:HEADS": HEADS}
#: SigLIP2's towers, each one layer of 32: a picture is 1024 tokens of 32.
TOKEN = 32
_TOWER: dict[str, Json] = {"hidden_size": TOKEN, "intermediate_size": 64, "num_hidden_layers": 1, "num_attention_heads": 2}
#: The IP-Adapter's LoRA rank (upstream's 32, alpha 32).
RANK = 4
_MODELSPEC = {"modelspec.architecture": "anima-preview/control-net-lllite",
              "modelspec.implementation": "https://huggingface.co/circlestone-labs/Anima",
              "modelspec.prediction_type": "epsilon", "modelspec.resolution": "1024x1024", "modelspec.sai_model_spec": "1.0.1"}
#: kohya-ss/Anima-LLLite's files' `__metadata__` (their headers), by the release each is.
_LLLITE: dict[str, dict[str, str]] = {
    "kohya-ss/lllite-depth@0.1.0": {
        "lllite.cond_dim": "32", "lllite.cond_emb_dim": "32", "lllite.cond_resblocks": "3", "lllite.mlp_dim": "32",
        "lllite.target_atomics": "self_attn_q_pre", "lllite.target_layers": "self_attn_q_pre", "lllite.use_aspp": "false",
        "lllite.version": "2", "modelspec.title": "anima-lllite-depth-1"},
    "kohya-ss/lllite-pose@0.1.0": {
        "lllite.cond_dim": "32", "lllite.cond_emb_dim": "32", "lllite.cond_resblocks": "3", "lllite.mlp_dim": "32",
        "lllite.target_atomics": "self_attn_q_pre,self_attn_kv_pre", "lllite.target_layers": "self_attn_q_pre,self_attn_kv_pre",
        "lllite.use_aspp": "false", "lllite.version": "2", "modelspec.title": "anima-lllite-pose-3r"},
    "kohya-ss/lllite-any-test-like@0.1.0": {
        "lllite.cond_dim": "64", "lllite.cond_emb_dim": "32", "lllite.cond_in_channels": "3", "lllite.cond_resblocks": "6",
        "lllite.inpaint_masked_input": "false", "lllite.mlp_dim": "64", "lllite.target_atomics": "self_attn_q_pre",
        "lllite.target_layers": "self_attn_q_pre", "lllite.use_aspp": "false", "lllite.version": "2",
        "modelspec.title": "anima-lllite-anytest-like-base-1r3-lr3e-4"},
    "kohya-ss/lllite-inpainting@0.1.0": {
        "lllite.cond_dim": "128", "lllite.cond_emb_dim": "64", "lllite.cond_in_channels": "4", "lllite.cond_resblocks": "4",
        "lllite.inpaint_masked_input": "true", "lllite.mlp_dim": "64",
        "lllite.target_atomics": "self_attn_q_pre,self_attn_kv_pre,mlp_fc1_pre",
        "lllite.target_layers": "self_attn_q_pre,self_attn_kv_pre,mlp_fc1_pre", "lllite.use_aspp": "false",
        "lllite.version": "2", "modelspec.title": "anima-lllite-inpainting-base-1"},
}
#: Each kohya target's linears, as its shifts are named (`lllite_dit_blocks_<block>_<linear>`).
_TARGETS = {"self_attn_q_pre": ("self_attn_q_proj",), "self_attn_kv_pre": ("self_attn_k_proj", "self_attn_v_proj"),
            "cross_attn_q_pre": ("cross_attn_q_proj",), "mlp_fc1_pre": ("mlp_layer1",)}


def _config(name: str) -> dict[str, Json]:
    loaded: dict[str, Json] = json.loads((packages() / "anima" / "configs" / f"{name}.json").read_text())
    return {k: v for k, v in loaded.items() if not k.startswith("_")}


def _siglip(out: Path) -> None:
    config: dict[str, Json] = {
        "initializer_factor": 1.0, "model_type": "siglip",
        "text_config": {"model_type": "siglip_text_model", "vocab_size": 256, **_TOWER},
        "transformers_version": "4.49.0.dev0",
        "vision_config": {"image_size": 512, "model_type": "siglip_vision_model", **_TOWER},
    }
    torch.manual_seed(3)
    encoder = module(SiglipModel(SiglipConfig.from_dict(config)))
    out.mkdir(parents=True)
    save_file({k: v.contiguous() for k, v in encoder.state_dict().items()}, out / "model.safetensors", metadata={"format": "pt"})
    (out / "config.json").write_text(json.dumps(config, indent=2))
    (out / "preprocessor_config.json").write_text(json.dumps({
        "do_convert_rgb": None, "do_normalize": True, "do_rescale": True, "do_resize": True, "image_mean": [0.5, 0.5, 0.5],
        "image_processor_type": "SiglipImageProcessor", "image_std": [0.5, 0.5, 0.5], "processor_class": "SiglipProcessor",
        "resample": 2, "rescale_factor": 1 / 255, "size": {"height": 512, "width": 512}}, indent=2))


class _File:
    """An author's single file as uploaded: one `model` component, no class or config, its `__metadata__` kept."""

    def __init__(self, seed: int) -> None:
        self.generator = torch.Generator().manual_seed(seed)
        self.tensors: dict[str, torch.Tensor] = {}

    def put(self, key: str, *shape: int, around: float = 0.0) -> None:
        self.tensors[key] = around + torch.randn(shape, generator=self.generator) * 0.1

    def linear(self, key: str, outputs: int, inputs: int, *kernel: int) -> None:
        self.put(f"{key}.weight", outputs, inputs, *kernel)
        self.put(f"{key}.bias", outputs)

    def norm(self, key: str, channels: int) -> None:
        self.put(f"{key}.weight", channels, around=1.0)
        self.put(f"{key}.bias", channels)

    def commit(self, store: Path, metadata: dict[str, str]) -> str:
        writer = tensorfs.Writer(cache_dir=store)
        component = writer.component("model")
        for key, tensor in self.tensors.items():
            data = tensor.to(torch.bfloat16).contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype="BF16", shape=list(data.shape))
        writer.metadata["safetensors_metadata"] = {"model": metadata}
        return writer.commit().id


def _ip_adapter(store: Path) -> str:
    """LuciferTC's ip_adapter-Character_Reference-10: each block's image keys and values from SigLIP2's tokens, its gate,
    and its PEFT LoRA over self-attention, cross-attention and the MLP (the package applies the cross-attention's)."""
    # The cross-attention's keys and values read the text conditioner's WIDTH.
    sites = {"self_attn.q_proj": (DIT, DIT), "self_attn.k_proj": (DIT, DIT), "self_attn.v_proj": (DIT, DIT),
             "self_attn.output_proj": (DIT, DIT), "cross_attn.q_proj": (DIT, DIT), "cross_attn.k_proj": (WIDTH, DIT),
             "cross_attn.v_proj": (WIDTH, DIT), "cross_attn.output_proj": (DIT, DIT), "mlp.layer1": (DIT, 4 * DIT),
             "mlp.layer2": (4 * DIT, DIT)}
    file = _File(5)
    for block in range(BLOCKS):
        file.linear(f"blocks.{block}.adaln_ip.1", DIT, DIT)
        file.linear(f"blocks.{block}.ip_k_proj", DIT, TOKEN)
        file.linear(f"blocks.{block}.ip_v_proj", DIT, TOKEN)
        for site, (inputs, outputs) in sites.items():
            file.put(f"lora.base_model.model.blocks.{block}.{site}.lora_A.weight", RANK, inputs)
            file.put(f"lora.base_model.model.blocks.{block}.{site}.lora_B.weight", outputs, RANK)
    return file.commit(store, {"format": "pt", "ip_inject_before_mlp": "False", "ip_norm_keys": "False",
                               "lora_alpha": str(RANK), "lora_rank": str(RANK)})


def _lllite(store: Path, metadata: dict[str, str], seed: int) -> str:
    """A kohya ControlNet-LLLite v2 file: the map's trunk (`lllite_conditioning1`) and each target linear's shift, as
    its metadata states them."""
    said = {k.removeprefix("lllite."): v for k, v in metadata.items()}
    channels = int(said.get("cond_in_channels", "3"))
    width, emb, mlp = (int(said[k]) for k in ("cond_dim", "cond_emb_dim", "mlp_dim"))
    file, trunk, half = _File(seed), "lllite_conditioning1", width // 2
    file.linear(f"{trunk}.conv1", half, channels, 4, 4)
    file.norm(f"{trunk}.norm1", half)
    file.linear(f"{trunk}.conv2", half, half, 3, 3)
    file.norm(f"{trunk}.norm2", half)
    file.linear(f"{trunk}.conv3", width, half, 4, 4)
    file.norm(f"{trunk}.norm3", width)
    for index in range(int(said["cond_resblocks"])):
        for part in ("1", "2"):
            file.norm(f"{trunk}.resblocks.{index}.norm{part}", width)
            file.linear(f"{trunk}.resblocks.{index}.conv{part}", width, width, 3, 3)
    file.linear(f"{trunk}.proj", emb, width, 1, 1)
    file.norm(f"{trunk}.out_norm", emb)
    for block in range(BLOCKS):
        for atomic in said["target_atomics"].split(","):
            for linear in _TARGETS[atomic]:
                shift = f"lllite_dit_blocks_{block}_{linear}"
                file.linear(f"{shift}.down", mlp, DIT)
                file.linear(f"{shift}.mid", mlp, mlp + emb)
                file.linear(f"{shift}.cond_to_film", 2 * mlp, emb)
                file.linear(f"{shift}.up", DIT, mlp)
                file.put(f"{shift}.depth_embed", emb)
    return file.commit(store, {**_MODELSPEC, **metadata})


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    out = root / "anima"
    text = {**_config("text_encoder"), "hidden_size": WIDTH, "intermediate_size": 128, "num_hidden_layers": 2,
            "num_attention_heads": 4, "num_key_value_heads": 2, "head_dim": 16, "max_window_layers": 2,
            "layer_types": ["full_attention"] * 2}
    save(module(Qwen3Model(Qwen3Config.from_dict(text))).to(torch.bfloat16), out / "text_encoder")
    save(module(AnimaTextConditioner(**{**_config("text_conditioner"), "source_dim": WIDTH, "target_dim": WIDTH,
                                        "model_dim": WIDTH, "num_layers": 1, "num_attention_heads": 4})
                ).to(torch.bfloat16), out / "text_conditioner")
    save(module(CosmosTransformer3DModel(**{**_config("transformer"), "attention_head_dim": HEAD, "num_attention_heads": HEADS,
                                            "num_layers": BLOCKS, "adaln_lora_dim": 32, "crossattn_proj_in_channels": WIDTH,
                                            "encoder_hidden_states_channels": WIDTH, "text_embed_dim": WIDTH})
                ).to(torch.bfloat16), out / "transformer")
    save(module(AutoencoderKLQwenImage(**{**_config("vae"), "base_dim": 16, "num_res_blocks": 1})).to(torch.bfloat16),
         out / "vae")
    (out / "scheduler").mkdir()
    shutil.copyfile(packages() / "anima" / "configs" / "scheduler.json", out / "scheduler" / "scheduler_config.json")
    shutil.copyfile(packages() / "anima" / "configs" / "modular_model_index.json", out / "modular_model_index.json")
    # The original tokenizers travel with this source model, independent of package bundles.
    source = Path(__file__).with_name("assets") / "anima"
    for name in ("tokenizer", "t5_tokenizer"):
        shutil.copytree(source / name, out / name)
    _siglip(root / "siglip2")
    return {"circlestone-labs/anima@1.0.0": out, "google/siglip2-base-patch16-512@0.1.0": root / "siglip2",
            "lucifertc/ip-adapter@0.1.0": _ip_adapter(store),
            **{ref: _lllite(store, metadata, 11 + index) for index, (ref, metadata) in enumerate(_LLLITE.items())}}


def _hole(folder: Path) -> Path:
    """An inpaint map: a picture with the area to redraw, a disc, transparent."""
    y, x = np.mgrid[0:128, 0:128].astype(np.float32)
    alpha = np.where((x - 70) ** 2 + (y - 60) ** 2 < 36**2, 0.0, 255.0)
    path = folder / "hole.png"
    Image.fromarray(np.stack([x * 2, y * 2, np.full_like(x, 90.0), alpha], axis=-1).astype(np.uint8)).save(path)
    return path


_FOX = _shared.picture("fox", 160, 128)
_DEPTH = _shared.picture("depth", 128, 128)
_SMALL: dict[str, Json] = {"prompt": "masterpiece, a red fox in the snow", "steps": 2, "seed": 7, "megapixels": 1}
_MENTION: dict[str, Json] = {"text": "masterpiece, <fox> in the snow", "references": [{"key": "fox", "type": "image"}]}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": _SMALL, "check": size(1024, 1024)},
    {"entry": "generate", "name": "wide", "payload": {**_SMALL, "aspect_ratio": "16:9", "negative_prompt": "blurry"}},
    {"entry": "generate", "name": "step-cache", "payload": {**_SMALL, "steps": 4, "first_block_cache": 0.5}},
    {"entry": "image", "name": "tool", "payload": {"instructions": "a red fox in the snow", "megapixels": "1", "seed": 7}},
    # Image to image: the canvas the start picture's shape at 1 MP, half the schedule redrawing it.
    {"entry": "generate", "name": "image-to-image", "payload": {**_SMALL, "steps": 4, "strength": 0.5},
     "files": {"image": _FOX}, "check": size(1152, 928)},
    {"entry": "image", "name": "edit", "payload": {"instructions": "a red fox in the snow", "megapixels": "1", "seed": 7,
                                                   "strength": 0.5}, "files": {"image": _FOX}, "check": size(1152, 928)},
    # The inpaint map's area redrawn from noise, the rest put back each step; its adapter off.
    {"entry": "generate", "name": "inpaint-area", "payload": {**_SMALL, "control": [{"kind": "inpaint", "strength": 0.0}]},
     "files": {"control.0.image": _hole}, "check": size(1024, 1024)},
    {"entry": "generate", "name": "reference", "payload": {**_SMALL, "prompt": _MENTION},
     "files": {"prompt.references.0.file": _FOX}, "check": size(1024, 1024)},
    {"entry": "generate", "name": "depth", "payload": {**_SMALL, "control": [{"kind": "depth"}]},
     "files": {"control.0.image": _DEPTH}, "check": size(1024, 1024)},
    {"entry": "generate", "name": "inpaint", "payload": {**_SMALL, "control": [{"kind": "inpaint"}]},
     "files": {"control.0.image": _hole}, "check": size(1024, 1024)},
    quantize("circlestone-labs/anima@1.0.0", "fp8", "anima", "anima@0.6.1"),
]
