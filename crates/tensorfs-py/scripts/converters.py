#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "diffusers==0.40.0", "transformers==5.16.1", "torch>=2.11", "numpy",
#   "einops==0.8.2", "omegaconf==2.4.0", "rotary-embedding-torch==0.9.1",
# ]
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
# ///
"""Generate the converter tables (`crates/tensorfs/converters/*.json`) from upstream's converters.

    uv run scripts/converters.py           # regenerate every table and compare byte for byte
    uv run scripts/converters.py --write   # rewrite them

Each family runs its reference converter over stand-ins built from real headers in `vectors/`:
coordinate-tracking views for diffusers' single-file converters, row-index tensors for the
official MiniMax-H3 converter. Every non-rename row's ops, applied with numpy semantics (the
engine's), must reproduce the converter's own output on those stand-ins, and every table's
targets must be the constructor's census. Canonical names are upstream's on-disk names: SDXL
`text_encoder` keeps `text_model.`, H3 drops `rope.inv_freq` (diffusers' converter does) and its
transformers land as `transformer`/`transformer_ref`. Those are spec data below. Rows are in the
target's construction order (its `state_dict()`), never sorted (#624).
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import importlib.machinery
import importlib.metadata
import importlib.util
import io
import json
import math
import struct
import sys
import tempfile
import types
import urllib.request
import zipfile
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import numpy as np
import torch
from diffusers.loaders import lora_conversion_utils, single_file_utils
from diffusers.utils import state_dict_utils
from transformers import AutoConfig, AutoModelForImageTextToText

PINS = {"diffusers": "0.40.0", "transformers": "5.16.1"}
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT.parent / "tensorfs" / "converters"
VECTORS = ROOT.parent / "tensorfs" / "vectors"
HF = "https://huggingface.co"
H3_SCRIPT = (
    "https://raw.githubusercontent.com/huggingface/diffusers/c60830ee365d520ab52b110dda562dd26f7b4d7f/"
    "scripts/convert_minimax_h3_to_diffusers.py",
    "86f61f62934d1eccf2a76ec57b6d072e2c23767c6e49d96aa1cf24d40422b42c",
)
H3_REFERENCE = "hf://MiniMaxAI/MiniMax-H3@42ed227ee7df40d41602854ae760620d6eb651fe"
SEEDVR = "https://raw.githubusercontent.com/ByteDance-Seed/SeedVR/e4de8c24441a67e1b7df56abea10645059bb1185/"


def fetch(url: str, sha256: str | None = None) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as response:
        body = response.read()
    if sha256 and hashlib.sha256(body).hexdigest() != sha256:
        sys.exit(f"{url}: sha256 differs from the pinned {sha256}")
    return body


def header(path: Path) -> dict[str, tuple[str, tuple[int, ...]]]:
    raw = path.read_bytes()
    (length,) = struct.unpack("<Q", raw[:8])
    doc = json.loads(raw[8 : 8 + length])
    return {k: (v["dtype"], tuple(v["shape"])) for k, v in doc.items() if k != "__metadata__"}


def apply(a: np.ndarray, ops: dict[str, Any]) -> np.ndarray:
    """A row's ops with numpy semantics, which the engine implements."""
    if "view" in ops:
        a = a.reshape(ops["view"])
    if "take" in ops:
        a = np.take(a, ops["take"][1], axis=ops["take"][0])
    if "permute" in ops:
        a = a.transpose(ops["permute"])
    if "shape" in ops:
        a = a.reshape(ops["shape"])
    return a


def row(target: str, source: str, ops: dict[str, Any] | None = None) -> list[Any]:
    if ops:
        return [target, source, ops]
    return [target] if target == source else [target, source]


# ------------------------------------------------------------------ diffusers single-file


def contiguous(shape: tuple[int, ...]) -> tuple[int, ...]:
    return tuple(math.prod(shape[i + 1 :]) for i in range(len(shape)))


@dataclass(frozen=True)
class Materialized:
    op: str
    sources: tuple[str, ...]

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__"):
            raise AttributeError(name)
        return lambda *a, **k: Materialized(f"{self.op}.{name}", self.sources)


@dataclass(frozen=True)
class Route:
    """One source tensor's coordinates: shape, element strides and element offset."""

    key: str
    shape: tuple[int, ...]
    offset: int
    strides: tuple[int, ...]

    @property
    def ndim(self) -> int:
        return len(self.shape)

    @property
    def T(self) -> Route:
        assert self.ndim == 2
        return replace(self, shape=self.shape[::-1], strides=self.strides[::-1])

    def dim(self) -> int:
        return self.ndim

    def size(self, axis: int | None = None) -> Any:
        return self.shape if axis is None else self.shape[axis]

    def numel(self) -> int:
        return math.prod(self.shape)

    def contiguous(self) -> Route:
        return self

    clone = detach = contiguous

    def __getitem__(self, key: Any) -> Route:
        index = key if isinstance(key, tuple) else (key,)
        index = (*index, *([slice(None)] * (self.ndim - len(index))))
        shape, stride, offset = [], [], self.offset
        for size, step, item in zip(self.shape, self.strides, index, strict=True):
            if isinstance(item, int):
                offset += item * step
            else:
                start, stop, jump = item.indices(size)
                shape.append(len(range(start, stop, jump)))
                stride.append(step * jump)
                offset += start * step
        return replace(self, shape=tuple(shape), strides=tuple(stride), offset=offset)

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__"):
            raise AttributeError(name)
        return lambda *a, **k: Materialized(name, (self.key,))

    @classmethod
    def __torch_function__(cls, func: Any, types: Any, args: Any = (), kwargs: Any = None) -> Any:
        sources: list[str] = []

        def collect(v: Any) -> None:
            if isinstance(v, Route):
                sources.append(v.key)
            elif isinstance(v, list | tuple):
                for item in v:
                    collect(item)

        collect(args)
        return Materialized(getattr(func, "__name__", str(func)), tuple(sources))


def route_ops(route: Route, source: tuple[int, ...]) -> dict[str, Any] | None:
    """The row ops reproducing `route`, checked element for element."""
    size = math.prod(route.shape)
    if route.strides == contiguous(route.shape) and route.offset % size == 0:
        if route.shape == source:
            ops = None
        elif size == math.prod(source):
            ops = {"shape": list(route.shape)}
        else:
            parts = math.prod(source) // size
            ops = {"view": [parts, -1], "take": [0, [route.offset // size]], "shape": list(route.shape)}
    elif route.ndim == 2 and route.offset == 0 and route.shape == source[::-1]:
        ops = {"permute": [1, 0]}
    else:
        raise SystemExit(f"{route.key}: a layout no row op expresses")
    flat = np.arange(math.prod(source))
    want = np.lib.stride_tricks.as_strided(flat[route.offset :], route.shape, [s * 8 for s in route.strides])
    got = flat.reshape(source) if ops is None else apply(flat.reshape(source), ops)
    assert np.array_equal(got, want), route.key
    return ops


def reference_config(reference: str, path: str) -> dict[str, Any]:
    repository, revision = reference.removeprefix("hf://").split("@")
    return json.loads(fetch(f"{HF}/{repository}/resolve/{revision}/{path}"))


def construct(model: str, config: dict[str, Any]) -> Any:
    library, name = model.split(":")
    kind = getattr(importlib.import_module(library), name)
    with torch.device("meta"):
        return kind.from_config(config) if library == "diffusers" else kind(kind.config_class(**config))


SINGLE_FILE = {
    "sdxl.single_file": {
        "reference": "hf://stabilityai/stable-diffusion-xl-base-1.0@462165984030d82259a11f4367a4eed129e94a7b",
        "header": "civitai-sdxl-headers/128078-92696.safetensors.header",
        "cast": "F16",
        "components": [
            # Transformers 5's CLIPTextModel drops `text_model.`; on disk it is there (D4).
            {"name": "text_encoder", "prefix": "conditioner.embedders.0.transformer.", "model": "transformers:CLIPTextModel",
             "config": "text_encoder/config.json", "convert": "convert_ldm_clip_checkpoint", "arguments": ["checkpoint"],
             "census": {"text_model.": ""}, "discard": ["text_model.embeddings.position_ids"],
             "optional": ["text_model.embeddings.position_ids"], "upstream": "sdxl-headers/text_encoder__model.fp16.safetensors.header"},
            {"name": "text_encoder_2", "prefix": "conditioner.embedders.1.model.", "model": "transformers:CLIPTextModelWithProjection",
             "config": "text_encoder_2/config.json", "convert": "convert_open_clip_checkpoint", "arguments": ["model", "checkpoint", "prefix"],
             # Some community checkpoints (Juggernaut XL v9) carry no `logit_scale`: the model never reads it.
             "optional": ["logit_scale", "transformer.text_model.embeddings.position_ids"], "upstream": "sdxl-headers/text_encoder_2__model.fp16.safetensors.header"},
            {"name": "unet", "prefix": "model.diffusion_model.", "model": "diffusers:UNet2DConditionModel", "config": "unet/config.json",
             "convert": "convert_ldm_unet_checkpoint", "arguments": ["checkpoint", "config"],
             "upstream": "sdxl-headers/unet__diffusion_pytorch_model.fp16.safetensors.header"},
            {"name": "vae", "prefix": "first_stage_model.", "model": "diffusers:AutoencoderKL", "config": "vae/config.json",
             "convert": "convert_ldm_vae_checkpoint", "arguments": ["checkpoint", "config"],
             "upstream": "sdxl-headers/vae__diffusion_pytorch_model.fp16.safetensors.header"},
        ],
    },
    "anima.single_file": {
        "reference": "hf://circlestone-labs/Anima-Base-v1.0-Diffusers@073c3a9db359c31ad0e8aa268d15775473c2176c",
        "cast": "BF16",
        "components": [
            {"name": "text_conditioner", "header": "anima-civitai-headers/2945208-2824391.safetensors.header", "prefix": "net.llm_adapter.",
             "model": "diffusers:AnimaTextConditioner", "config": "text_conditioner/config.json",
             "upstream": "anima-hf-headers/text_conditioner/diffusion_pytorch_model.safetensors.header"},
            {"name": "text_encoder", "header": "anima-civitai-headers/2945208-2824387.safetensors.header", "prefix": "model.",
             "model": "transformers:Qwen3Model", "config": "text_encoder/config.json",
             "upstream": "anima-hf-headers/text_encoder/model.safetensors.header"},
            {"name": "transformer", "header": "anima-civitai-headers/2945208-2824391.safetensors.header", "prefix": "net.",
             "model": "diffusers:CosmosTransformer3DModel", "config": "transformer/config.json",
             "convert": "convert_cosmos_transformer_checkpoint_to_diffusers", "arguments": ["checkpoint"],
             "upstream": "anima-hf-headers/transformer/diffusion_pytorch_model.safetensors.header"},
            {"name": "vae", "header": "anima-civitai-headers/2945208-2004692.safetensors.header", "prefix": "",
             "model": "diffusers:AutoencoderKLQwenImage", "config": "vae/config.json", "convert": "convert_wan_vae_to_diffusers",
             "arguments": ["checkpoint"], "upstream": "anima-hf-headers/vae/diffusion_pytorch_model.safetensors.header"},
        ],
    },
}


def single_file(name: str) -> tuple[dict[str, Any], dict[str, dict[str, tuple[int, ...]]]]:
    """The table, and each component's linears and convolutions with their weight shapes (the
    LoRA tables' targets)."""
    spec = SINGLE_FILE[name]
    prefixes = [c["prefix"] for c in spec["components"]]
    components, shapes, layers = [], {}, {}
    for c in spec["components"]:
        prefix = c["prefix"]
        source = header(VECTORS / c.get("header", spec.get("header")))
        checkpoint = {
            k: Route(k, shape, 0, contiguous(shape))
            for k, (_, shape) in source.items()
            if max((p for p in prefixes if k.startswith(p)), key=len, default=None) == prefix
        }
        config = reference_config(spec["reference"], c["config"])
        model = construct(c["model"], config)
        arguments = {"checkpoint": checkpoint, "config": config, "model": model, "prefix": prefix}
        if c.get("convert"):
            converted = getattr(single_file_utils, c["convert"])(*(arguments[a] for a in c["arguments"]))
        else:
            converted = {k.removeprefix(prefix): r for k, r in checkpoint.items()}
        census_of = c.get("census", {})

        def on_disk(census_key: str) -> str:
            for disk, census in census_of.items():
                if census_key.startswith(census):
                    return disk + census_key.removeprefix(census)
            return census_key

        rows, used, shapes[c["name"]] = [], set(), {}
        for key, value in model.state_dict().items():
            target = on_disk(key)
            route = converted.pop(target)
            assert isinstance(route, Route), (name, target, route)
            assert route.shape == tuple(value.shape), (name, target)
            local = route.key.removeprefix(prefix)
            used.add(local)
            rows.append(row(target, local, route_ops(route, source[route.key][1])))
            shapes[c["name"]][target] = route.shape
        assert sorted(converted) == sorted(c.get("discard", [])), (name, c["name"], sorted(converted))
        upstream = header(VECTORS / c["upstream"])
        assert {t: tuple(s) for t, s in shapes[c["name"]].items()} == {k: s for k, (_, s) in upstream.items()}, (
            name, c["name"], "targets are not upstream's on-disk names and shapes")
        layers[c["name"]] = {
            on_disk(f"{path}.weight").removesuffix(".weight"): shapes[c["name"]][on_disk(f"{path}.weight")]
            for path, module in model.named_modules() if isinstance(module, (torch.nn.Linear, torch.nn.Conv2d))
        }
        present = {k.removeprefix(prefix) for k in checkpoint}
        optional = sorted(c.get("optional", []))
        components.append({"name": c["name"], "prefix": prefix, "cast": spec["cast"],
                           "drop": sorted(present - used - set(optional)), "optional": optional, "rows": rows})
    return {"name": name, "reference": spec["reference"], "components": components}, layers


# ------------------------------------------------------------------ MiniMax-H3


def h3_converter() -> Any:
    body = fetch(*H3_SCRIPT)
    with tempfile.NamedTemporaryFile("wb", suffix=".py", delete=False) as f:
        f.write(body)
    spec = importlib.util.spec_from_file_location("convert_minimax_h3_to_diffusers", f.name)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    Path(f.name).unlink()
    return module


def row_ops(rows: np.ndarray, source: tuple[int, ...]) -> dict[str, Any] | None:
    """The fewest-member view/take whose output rows are `rows` of a `source`-shaped tensor."""
    count, out = source[0], len(rows)
    rest = [-1] if len(source) > 1 else []
    if out == count and np.array_equal(rows, np.arange(count)):
        return None
    unit = next((i for i in range(1, out) if rows[i] != rows[i - 1] + 1), out)
    stand_in = np.arange(count * 2).reshape(count, 2) if rest else np.arange(count)
    blocks = count // unit
    for members in range(2, blocks + 1):
        if count % unit or blocks % members or out % (blocks // members * unit):
            continue
        groups = blocks // members
        picked = [int(b) % members for b in rows[::unit][: out // (groups * unit)] // unit]
        view = [members, -1] if groups == 1 else [groups, members, -1]
        ops = {"view": view, "take": [len(view) - 2, picked], "shape": [out, *rest]}
        if np.array_equal(apply(stand_in, ops), stand_in[rows]):
            return ops
    raise SystemExit(f"rows of {source}: no view/take expresses them")


def h3_index(rows: int) -> torch.Tensor:
    return torch.arange(rows).reshape(rows, 1)


def h3_native(h3: Any, comfy: bool = False) -> tuple[dict[str, Any], dict[str, tuple[str, tuple[int, ...]]]]:
    """The native transformer table, and the native header (for the LoRA). `comfy`: ComfyUI's
    layout of the same keys, which holds each fused qkv as [q_all; k_all; v_all]
    (comfy/ldm/minimax/model.py splits it contiguously, and nothing reorders it on load) where the
    official shards interleave it per head; everything else is the same."""
    config = h3.MINIMAX_H3_TRANSFORMER_CONFIG
    heads, head_dim = config["num_attention_heads"], config["attention_head_dim"]
    from diffusers import MiniMaxH3Transformer3DModel

    with torch.device("meta"):
        census = {k: tuple(v.shape) for k, v in MiniMaxH3Transformer3DModel(**config).state_dict().items()}
    variants = {}
    for variant in ("FL2VA", "Ref2VA"):
        merged: dict[str, tuple[str, tuple[int, ...]]] = {}
        for shard in sorted((VECTORS / "h3-headers").glob(f"{variant}__transformer__model-*.safetensors")):
            merged.update(header(shard))
        variants[variant] = merged
    source = variants["FL2VA"]
    assert {k: s for k, (_, s) in variants["Ref2VA"].items()} == {k: s for k, (_, s) in source.items()}
    mapped, dropped, expect = {}, [], {}
    for key, (_, shape) in source.items():
        stand_in = h3_index(shape[0]) if len(shape) > 1 else torch.arange(shape[0])
        if key.endswith(".attn.qkv_proj.weight"):
            if not comfy:
                stand_in = h3.reorder_interleaved_qkv(stand_in, heads, head_dim)
            norm = key.replace("qkv_proj.weight", "q_norm.weight")
            expect[norm] = [head_dim]
            assert source[norm][1] == (head_dim,), norm
        outputs = h3.convert_transformer_key(key, stand_in, config)
        if not outputs:
            dropped.append(key)
        for target, tensor in outputs:
            ops = row_ops(tensor.reshape(tensor.shape[0], -1)[:, 0].numpy(), shape)
            if ops:
                expect[key] = list(shape)
            mapped[target] = row(target, key, ops)
            got = apply(np.zeros(shape, np.int8), ops) if ops else np.zeros(shape, np.int8)
            assert got.shape == census[target], (target, got.shape, census[target])
    assert sorted(mapped) == sorted(census), "targets are not the transformer's census"
    rows = [mapped[k] for k in census]
    if comfy:
        # ComfyUI saves a DiT alone under its checkpoint prefix (Singularity V2).
        component = {"name": "transformer", "prefix": "model.diffusion_model.",
                     "drop": sorted(dropped), "expect": dict(sorted(expect.items())), "rows": rows}
        return {"name": "h3.comfy", "reference": H3_REFERENCE, "components": [component]}, source
    component = {"name": "transformer", "folders": {"FL2VA/transformer": "transformer", "Ref2VA/transformer": "transformer_ref"},
                 "drop": sorted(dropped), "expect": dict(sorted(expect.items())), "rows": rows}
    return {"name": "h3.native", "reference": H3_REFERENCE, "components": [component]}, source


# Who writes H3's fused Q/K/V LoRA factors as the trainers' contiguous thirds, by the keys and
# `__metadata__` their own code writes (docs/h3-lora-source-layout.md). Recognition chooses h3.lora
# only for these; any other source names it. Not here: fal's minimax-h3-trainer (no public code),
# DiffSynth-Studio (head-interleaved on MiniMax's raw base, `blocks.*` keys and `format: pt` only),
# convert_h3_lora.py and MidLAB (no public code).
GH = "https://github.com"
H3_LORA_PRODUCERS = [
    {"name": "ai-toolkit", "keys": ["diffusion_model."], "metadata": {"software": {"json": {"name": "ai-toolkit"}}}, "cite": [
        f"{GH}/ostris/ai-toolkit/blob/b69b98314557f7c8b08e11bbb14e48d959e6ab26/extensions_built_in/diffusion_models/minimax_h3/src/transformer.py#L184",
        f"{GH}/ostris/ai-toolkit/blob/b69b98314557f7c8b08e11bbb14e48d959e6ab26/extensions_built_in/diffusion_models/minimax_h3/minimax_h3.py#L1113",
        f"{GH}/ostris/ai-toolkit/blob/b69b98314557f7c8b08e11bbb14e48d959e6ab26/toolkit/metadata.py#L20"]},
    # Its own keys, and `convert_lora.py --target other`'s, which keep its metadata.
    {"name": "musubi-tuner", "keys": ["lora_unet_", "diffusion_model."], "metadata": {"ss_network_module": "networks.lora_minimax_h3"}, "cite": [
        f"{GH}/kohya-ss/musubi-tuner/blob/f8a1b03794a49239a3539015075f5123d6c07d66/src/musubi_tuner/minimax_h3/model.py#L436",
        f"{GH}/kohya-ss/musubi-tuner/blob/f8a1b03794a49239a3539015075f5123d6c07d66/src/musubi_tuner/convert_lora.py#L143-L154",
        f"{GH}/AkaneTendo25/musubi-tuner/blob/0499c2f8105994bc98ea912dd46ff5ae1ac2628c/src/musubi_tuner/minimax_h3/model.py#L535",
        f"{GH}/AkaneTendo25/musubi-tuner/blob/0499c2f8105994bc98ea912dd46ff5ae1ac2628c/src/musubi_tuner/networks/lora.py#L1132-L1143"]},
    # `lora_format: comfyui`: Diffusers' separate Q/K/V stacked into thirds.
    {"name": "SimpleTuner", "keys": ["diffusion_model."], "metadata": {"modelspec.architecture": {"prefix": "minimaxh3-"}, "modelspec.implementation": "diffusers"}, "cite": [
        f"{GH}/bghira/SimpleTuner/blob/4b7edb115f95f5bae8855da5dd8e68242eea6922/simpletuner/helpers/models/minimaxh3/modular_pipeline.py#L332-L443"]},
    # Its MiniMax trainers' LoRAs (not `fizgig.model_eq`, a diff of whatever two checkpoints it is given).
    *({"name": "Fizgig", "keys": ["lora_unet_"], "metadata": {"ss_network_module": {"prefix": prefix}}, "cite": [
        f"{GH}/shootthesound/Fizgig/blob/15c8447941842aeb8c8472daf0ea627f0034fbb8/src/fizgig/minimax/model.py#L361",
        f"{GH}/shootthesound/Fizgig/blob/15c8447941842aeb8c8472daf0ea627f0034fbb8/src/fizgig/families/minimax.py#L359-L365"]}
      for prefix in ("fizgig.minimax", "fizgig.families (minimax")),
    # Its converter is not public; the file states the fusion of Diffusers' separate Q/K/V.
    {"name": "lightx2v", "keys": ["diffusion_model."], "metadata": {"source_format": "Diffusers PEFT LoRA", "qkv_fusion": "block diagonal B; concat A; alpha multiplied by 3"}, "cite": [
        "https://huggingface.co/lightx2v/Minimax-h3-Turbo/blob/30f485ed874aa44c8183348ad361c5eabad318da/minimax_h3_fl2v_turbo_8step_v1.0_768p_comfyui_bf16.safetensors"]},
]


def h3_lora(h3: Any, source: dict[str, tuple[str, tuple[int, ...]]]) -> dict[str, Any]:
    """Trainer LoRA factors: A as is, B through diffusers' own transformer conversion given the
    trainer's contiguous Q/K/V thirds (docs/h3-lora-source-layout.md), so fc1 B halves swap."""
    config = h3.MINIMAX_H3_TRANSFORMER_CONFIG
    leaves = {"attn.qkv_proj": "attn_qkv_proj", "attn.out_proj": "attn_out_proj", "mlp.fc1": "mlp_fc1", "mlp.fc2": "mlp_fc2"}
    blocks = [("blocks", i, True) for i in range(config["num_layers"])]
    blocks += [("token_refiner.blocks", i, False) for i in range(config["num_refiner_layers"])]
    modules = [(f"{family}.{i}.{leaf}", f"blocks_{i}_{flat}" if kohya else None)
               for family, i, kohya in blocks for leaf, flat in leaves.items()]
    # The audio patch projections, which audio-trained LoRAs adapt (Mystic XXX v5).
    modules += [("audio_patch_proj", "audio_patch_proj"), ("final_layer.audio_out", "final_layer_audio_out")]
    rows = []
    for native, flat in modules:
        stems = [f"diffusion_model.{native}"] + ([f"lora_unet_{flat}"] if flat else [])
        fused, inp = source[f"{native}.weight"][1]
        for target, b in h3.convert_transformer_key(f"{native}.weight", h3_index(fused), config):
            ops: dict[str, Any] = {"io": [inp, fused]}
            if b_ops := row_ops(b[:, 0].numpy(), (fused, 1)):
                ops["b"] = b_ops
            rows.extend([target.removesuffix(".weight"), stem, ops] for stem in stems)
    # Rows in the transformer's construction order (#624): its state_dict, not the trainer's.
    from diffusers import MiniMaxH3Transformer3DModel

    with torch.device("meta"):
        census = {k: i for i, k in enumerate(MiniMaxH3Transformer3DModel(**config).state_dict())}
    # Diffusers/PEFT trainers spell the target itself under `transformer.` (Open-H3-Regenerate-2K's
    # merged LoRA, diffusers' `save_lora_weights`).
    shapes = {k.removesuffix(".weight"): tuple(v) for k, v in census_shapes(config).items() if k.endswith(".weight")}
    for target in dict.fromkeys(r[0] for r in list(rows)):
        out, inp = shapes[target]
        rows.append([target, f"transformer.{target}", {"io": [inp, out]}])
    rows.sort(key=lambda r: census[r[0] + ".weight"])
    # The AdaLN curve: ComfyUI's pruned H3 computes each block's modulation from an 8-D curve of
    # the timestep (`lerp(adaln_t_table, t)`), so a LoRA trained on it adapts `adaln_proj` with
    # A [r, 8]. Its update, B·A·curve(t), depends on t alone: the curve site carries its factors
    # and the curve table it reads, and the serving side folds it into exact-timestep tables.
    blocks = config["num_layers"]
    width = shapes["transformer_blocks.0.adaln_proj.linear"][0]
    curve = {"io": [8, width], "table": "adaln_t_table"}
    for stem in ("transformer.transformer_blocks.{N}.adaln_proj.linear", "diffusion_model.blocks.{N}.adaln_proj.linear"):
        rows.append(["transformer_blocks.{N}.adaln_curve", stem, curve, {"N": blocks}])
    lora = {"a": [".lora_A.weight", ".lora_down.weight", ".curve_A.weight"],
            "b": [".lora_B.weight", ".lora_up.weight", ".curve_B.weight"], "alpha": [".alpha"]}
    return {"name": "h3.lora", "producers": H3_LORA_PRODUCERS, "lora": lora, "constants": {"adaln_t_table": adaln_t_table()},
            "components": [{"name": "transformer", "rows": rows}]}


def census_shapes(config: dict[str, Any]) -> dict[str, torch.Size]:
    from diffusers import MiniMaxH3Transformer3DModel

    with torch.device("meta"):
        return {k: v.shape for k, v in MiniMaxH3Transformer3DModel(**config).state_dict().items()}


# ComfyUI's pruned-AdaLN curve table: 1025 timesteps x 8, F32, read by byte range from its pinned file.
ADALN_T_TABLE = (
    "https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/e5eb578a89295337b8ff433a035929ce0279e0b6/"
    "diffusion_models/minimax_h3_ref2va_pruned_int8_convrot.safetensors",
    "adaln_t_table",
    "c02a6c11888297688c1e6278185ea1f947023acfc69f9003bbcdcec9a229a8e7",
)


def fetch_range(url: str, start: int, length: int) -> bytes:
    request = urllib.request.Request(url, headers={"Range": f"bytes={start}-{start + length - 1}"})
    with urllib.request.urlopen(request, timeout=120) as response:
        body = response.read()
    if response.status != 206 or len(body) != length:
        sys.exit(f"{url}: range {start}+{length} was not served")
    return body


def adaln_t_table() -> dict[str, Any]:
    import base64

    url, name, digest = ADALN_T_TABLE
    (length,) = struct.unpack("<Q", fetch_range(url, 0, 8))
    entry = json.loads(fetch_range(url, 8, length))[name]
    begin, end = entry["data_offsets"]
    body = fetch_range(url, 8 + length + begin, end - begin)
    if hashlib.sha256(body).hexdigest() != digest or entry["dtype"] != "F32" or entry["shape"] != [1025, 8]:
        sys.exit(f"{url}: {name} is not the pinned F32 [1025, 8] table")
    return {"dtype": "F32", "shape": [1025, 8], "sha256": digest, "base64": base64.b64encode(body).decode()}


# ------------------------------------------------------------------ SDXL LoRA

SDXL_LORA = {"unet": "lora_unet_", "text_encoder": "lora_te1_", "text_encoder_2": "lora_te2_"}


def kohya_reading(prefix: str, stems: list[str], unet: Any) -> dict[str, str]:
    """The module diffusers' SDXL loader adapts with each kohya `<prefix><stem>` factor pair
    (`lora_state_dict`'s conversion, then the PEFT renames its loaders apply), by stem: each
    stem's down factor carries its index through."""
    state = {}
    for i, stem in enumerate(stems):
        state[f"{prefix}{stem}.lora_down.weight"] = torch.full((1, 1), float(i))
        state[f"{prefix}{stem}.lora_up.weight"] = torch.zeros(1, 1)
        state[f"{prefix}{stem}.alpha"] = torch.tensor(1.0)
    state = lora_conversion_utils._maybe_map_sgm_blocks_to_diffusers(state, unet)
    state, _ = lora_conversion_utils._convert_non_diffusers_lora_to_diffusers(state)
    component = next(iter(state)).partition(".")[0]
    local = {k.partition(".")[2]: v for k, v in state.items()}
    if component == "unet":
        peft = state_dict_utils.convert_unet_state_dict_to_peft(local)
    else:
        peft = state_dict_utils.convert_state_dict_to_peft(state_dict_utils.convert_state_dict_to_diffusers(local))
    return {stems[int(v.item())]: k.removesuffix(".lora_A.weight") for k, v in peft.items() if k.endswith(".lora_A.weight")}


def sdxl_lora(table: dict[str, Any], layers: dict[str, dict[str, tuple[int, ...]]], unet: Any) -> dict[str, Any]:
    """Kohya SDXL factors onto every linear and convolution of the single-file table's UNet (its
    LDM or Diffusers spelling) and CLIP towers. A spelling is a row where diffusers' own loader
    reads it as that module (`kohya_reading`); one it reads as no module (a Diffusers-spelled
    `add_embedding`) is not."""
    components = []
    for c in table["components"]:
        if c["name"] not in SDXL_LORA:
            continue
        prefix, modules = SDXL_LORA[c["name"]], layers[c["name"]]
        sources = {r[0].removesuffix(".weight"): (r[1] if len(r) > 1 else r[0]).removesuffix(".weight") for r in c["rows"]}
        spellings = [{m.replace(".", "_"): m for m in modules}]
        if c["name"] == "unet":
            spellings.append({sources[m].replace(".", "_"): m for m in modules})
        stems: dict[str, list[str]] = {m: [] for m in modules}
        for spelled in spellings:
            for stem, read in kohya_reading(prefix, list(spelled), unet).items():
                assert read == spelled[stem] or read not in modules, (prefix, stem, spelled[stem], read)
                if read == spelled[stem] and stem not in stems[read]:
                    stems[read].append(stem)
        rows = []
        for module, shape in modules.items():
            ops = {"io": [shape[1], shape[0]]} | ({"kernel": list(shape[2:])} if len(shape) == 4 else {})
            rows.extend([module, stem, ops] for stem in stems[module])
        components.append({"name": c["name"], "prefix": prefix, "rows": rows})
    lora = {"a": [".lora_down.weight", ".lora_A.weight"], "b": [".lora_up.weight", ".lora_B.weight"], "alpha": [".alpha"]}
    return {"name": "sdxl.lora", "lora": lora, "components": components}


# ------------------------------------------------------------------ Krea 2 LoRA

KREA2 = VECTORS / "krea2-headers"


def krea2_lora() -> dict[str, Any]:
    """Krea 2 LoRA factors onto every linear of the DiT, in each spelling a published loader reads:
    the reference checkpoint's names (`krea2_raw_bf16`, Krea's trainer) under `diffusion_model.` (ComfyUI,
    AI-Toolkit) or `base_model.model.` (AI-Toolkit's PEFT), each one where diffusers' own conversion reads
    it as that module; Diffusers' names under `transformer.` (Diffusers and PEFT: Krea's LoRAs); and
    ComfyUI's further spellings (`comfy/lora.py`, `krea2_to_diffusers`): Diffusers' names under
    `diffusion_model.` or bare, `to_out` for `to_out.0`, kohya's flattened `lora_unet_` (musubi-tuner) and
    `lycoris_`, and the reference names bare."""
    from diffusers import Krea2Transformer2DModel

    with torch.device("meta"):
        model = Krea2Transformer2DModel()  # the class defaults are Krea 2's config
    census = {n: (m.in_features, m.out_features) for n, m in model.named_modules() if isinstance(m, torch.nn.Linear)}
    source = header(KREA2 / "krea2_raw_bf16.safetensors.header")
    stand_in = torch.zeros(1, 1)
    reference: dict[str, str] = {}  # Diffusers name -> reference name
    for key, (_, shape) in source.items():
        if not key.endswith(".weight") or len(shape) != 2:
            continue
        stem = key.removesuffix(".weight")
        for prefix in ("diffusion_model.", "base_model.model."):
            read = lora_conversion_utils._convert_non_diffusers_krea2_lora_to_diffusers(
                {f"{prefix}{stem}.lora_A.weight": stand_in, f"{prefix}{stem}.lora_B.weight": stand_in})
            target = next(iter(read)).removeprefix("transformer.").removesuffix(".lora_A.weight")
            assert census[target] == shape[::-1], (stem, target, shape)
            assert reference.setdefault(target, stem) == stem, target
    assert sorted(reference) == sorted(census), "the reference linears are not the DiT's census"

    def spelled(diffusers: str) -> list[str]:
        return [diffusers, diffusers.removesuffix(".0")] if diffusers.endswith(".to_out.0") else [diffusers]

    spellings = {
        "diffusion_model.": lambda d: [reference[d], *spelled(d)],
        "base_model.model.": lambda d: [reference[d]],
        "transformer.": spelled,
        "lora_unet_": lambda d: [reference[d].replace(".", "_")],
        "lycoris_": lambda d: [s.replace(".", "_") for s in spelled(d)],
        "": lambda d: [reference[d], *spelled(d)],
    }
    components = [
        {"name": "transformer", "prefix": prefix,
         "rows": [[d, stem, {"io": list(io)}] for d, io in census.items() for stem in spell(d)]}
        for prefix, spell in spellings.items()
    ]
    # Krea's own (Diffusers) and AI-Toolkit's published LoRAs are spelled as rows say.
    for vector in ("darkbrush.safetensors.header", "krea2_style_reference.safetensors.header"):
        keys = header(KREA2 / vector)
        for c in components:
            stems = {r[1] for r in c["rows"]}
            mine = [k for k in keys if k.startswith(c["prefix"])] if c["prefix"] else []
            assert all(k[len(c["prefix"]):].rsplit(".", 2)[0] in stems for k in mine), (vector, c["prefix"])
        assert any(k.startswith(c["prefix"]) for c in components[:-1] for k in keys), vector
    lora = {"a": [".lora_A.weight", ".lora_down.weight"], "b": [".lora_B.weight", ".lora_up.weight"], "alpha": [".alpha"]}
    return {"name": "krea2.lora", "lora": lora, "components": components}


# ------------------------------------------------------------------ SeedVR2


# The seedvr2 package's NaDiT and VAE arguments (SeedVR's configs_7b).
SEEDVR_DIT = dict(
    vid_in_channels=33, vid_out_channels=16, vid_dim=3072, txt_in_dim=5120, txt_dim=3072, emb_dim=18432,
    heads=24, head_dim=128, expand_ratio=4, norm="rms", norm_eps=1e-5, ada="single", qk_bias=False,
    qk_norm="rms", qk_rope=True, patch_size=(1, 2, 2), num_layers=36, mlp_type="normal", shared_mlp=False,
    shared_qkv=False, block_type=["mmdit_sr"] * 36, window=[(4, 3, 3)] * 36,
    window_method=["720pwin_by_size_bysize", "720pswin_by_size_bysize"] * 18,
)
SEEDVR_VAE = dict(
    act_fn="silu", block_out_channels=(128, 256, 512, 512), down_block_types=("DownEncoderBlock3D",) * 4,
    up_block_types=("UpDecoderBlock3D",) * 4, in_channels=3, out_channels=3, latent_channels=16,
    layers_per_block=2, norm_num_groups=32, slicing_sample_min_size=4, temporal_scale_num=2, inflation_mode="pad",
    spatial_downsample_factor=8, temporal_downsample_factor=4, use_quant_conv=False, use_post_quant_conv=False,
    freeze_encoder=False,
)


def seedvr_census() -> dict[str, list[str]]:
    """NaDiT's and the VAE's `state_dict()` order, built on the meta device from SeedVR's code at
    the pinned commit. Its attention module imports flash_attn, which construction never calls."""
    commit = SEEDVR.rstrip("/").rsplit("/", 1)[1]
    body = fetch(f"https://codeload.github.com/ByteDance-Seed/SeedVR/zip/{commit}")
    root = Path(tempfile.mkdtemp())
    zipfile.ZipFile(io.BytesIO(body)).extractall(root)
    sys.path.insert(0, str(root / f"SeedVR-{commit}"))
    stub = types.ModuleType("flash_attn")
    stub.__spec__ = importlib.machinery.ModuleSpec("flash_attn", None)
    stub.flash_attn_varlen_func = None
    sys.modules.setdefault("flash_attn", stub)
    from models.dit.nadit import NaDiT
    from models.video_vae_v3.modules.attn_video_vae import VideoAutoencoderKLWrapper

    with torch.device("meta"):
        return {"dit": list(NaDiT(**SEEDVR_DIT).state_dict()),
                "vae": list(VideoAutoencoderKLWrapper(**SEEDVR_VAE).state_dict())}


def seedvr2() -> dict[str, Any]:
    """The package's normalize composition: the two fp16 checkpoints as they are, and
    upstream's fixed text conditioning read straight from the pinned `.pt` files' storage
    (no unpickling). Rows follow NaDiT's/the VAE's construction order; the banked headers hold
    exactly their keys."""
    census = seedvr_census()
    components = []
    for name, file in (("dit", "seedvr2_ema_7b_fp16.safetensors"), ("vae", "ema_vae_fp16.safetensors")):
        keys = header(VECTORS / "seedvr2-headers" / f"{file}.header")
        assert sorted(keys) == sorted(census[name]), name
        components.append({"name": name, "cast": "F16", "rows": [[k] for k in census[name]]})
    rows = []
    for key, file, shape, digest in (
        ("positive", "pos_emb.pt", (58, 5120), "fa07a14844314772266b66c3b95deb0027696d8fe7065721263db5176f45d799"),
        ("negative", "neg_emb.pt", (64, 5120), "6a43e5800ef2354f1c156d27535834da055cbec8248298b8923492bba2076581"),
    ):
        body = fetch(SEEDVR + file, digest)
        with tempfile.NamedTemporaryFile(suffix=".pt") as f:
            f.write(body)
            f.flush()
            info = zipfile.ZipFile(f.name).getinfo(f"{file.removesuffix('.pt')}/data/0")
            tensor = torch.load(f.name, weights_only=True, map_location="cpu")
        (name_len, extra_len) = struct.unpack("<HH", body[info.header_offset + 26 : info.header_offset + 30])
        offset = info.header_offset + 30 + name_len + extra_len
        want = tensor.contiguous().view(torch.uint16).numpy().tobytes()
        assert info.compress_type == zipfile.ZIP_STORED and tuple(tensor.shape) == shape and tensor.dtype == torch.bfloat16
        assert body[offset : offset + len(want)] == want, file
        rows.append([key, {"sha256": digest, "length": len(body), "offset": offset, "dtype": "BF16", "shape": list(shape)}])
    components.append({"name": "conditioning", "rows": rows})
    return {"name": "seedvr2", "files": {"config.json": {"architecture": "SeedVR2-7B"}}, "components": components}


# ------------------------------------------------------------------ H3 latent upscaler

# LBH-123-AI's 3D-conv latent upscaler node, unchanged, as NVlabs' Open-H3-Regenerate-2K vendors it.
H3_UPSCALER_NODE = (
    "https://raw.githubusercontent.com/NVlabs/Sana/6e4a44002149418a547434b91d7280a8bbdcace3/"
    "models/Open-H3-Regenerate-2K/open_h3/upscaler_node.py",
    "f5945b500ddb38afee23c377034bf15394ba64748511a7e9608953040c4be437",
)


def h3_latent_upscaler() -> dict[str, Any]:
    """LBH's checkpoint as it is: rows in `LatentResizer3D`'s construction order, built from the
    node's own code (its ComfyUI imports stubbed), against the banked header's key set."""
    for name in ("folder_paths", "comfy", "comfy.model_management", "comfy_api", "comfy_api.latest"):
        sys.modules.setdefault(name, types.ModuleType(name))
    sys.modules["folder_paths"].folder_names_and_paths = {"latent_upscale_models": ([], set())}
    sys.modules["folder_paths"].models_dir = tempfile.mkdtemp()
    with tempfile.NamedTemporaryFile("wb", suffix=".py", delete=False) as f:
        f.write(fetch(*H3_UPSCALER_NODE))
    spec = importlib.util.spec_from_file_location("h3_upscaler_node", f.name)
    node = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(node)
    keys = header(VECTORS / "h3-latent-upscaler-headers" / "minimax_h3_latent_upscaler_3d_conv_v1_bf16.safetensors.header")
    config = node._detect_arch({k: torch.empty(shape, device="meta") for k, (_, shape) in keys.items()})
    with torch.device("meta"):
        census = list(node.LatentResizer3D(**config).state_dict())
    assert sorted(census) == sorted(keys), "the upscaler's header is not LatentResizer3D's state_dict"
    return {"name": "h3.latent_upscaler", "files": {"config.json": {"architecture": "minimax-h3-latent-upscaler-3d-conv-v1"}},
            "components": [{"name": "upscaler", "rows": [[k] for k in census]}]}


# ------------------------------------------------------------------ Ideogram 4

# Diffusers #14126's `convert_ideogram4_transformer_checkpoint_to_diffusers` (`attention.o` ->
# `to_out.0`, the fused `qkv` rows chunked into thirds), the single-file converter under review.
IDEOGRAM_CONVERTER = (
    "https://raw.githubusercontent.com/buffett0323/diffusers/1112a897779500cdefd36c4b5fec3bb83ac94db5/"
    "src/diffusers/loaders/single_file_utils.py",
    "0e334e75d6b539febc7ee8696bbf1a58434a9faf9c32cc646c303106c0cfc117",
)
# The transformer config of ideogram-ai/ideogram-4-fp8@ee79a72 (gated), read from an ungated copy whose
# blob is the same (git oid da6e1e85).
IDEOGRAM_CONFIG = (
    "https://huggingface.co/cocktailpeanut/ideogram-4-fp8/resolve/e0a0fb243cbf458fde53e358e80e6640cc593fd0/"
    "transformer/config.json",
    "da6e1e852ff7973df0f36a20d20bc55b5300f69b",
)


def ideogram4() -> dict[str, Any]:
    """Ideogram 4's official checkpoint: both DiTs in its original layout. Every linear's weight-only
    fp8 carrier (`.weight` + `.weight_scale`) is folded into one fp8.rowwise tensor before a table
    row reads it, so the scales are not rows here; `qkv`'s thirds are whole rows of it."""
    import ast

    tree = ast.parse(fetch(*IDEOGRAM_CONVERTER))
    (node,) = [n for n in tree.body if getattr(n, "name", "") == "convert_ideogram4_transformer_checkpoint_to_diffusers"]
    space: dict[str, Any] = {"torch": torch}
    exec(compile(ast.Module([node], []), "single_file_utils.py", "exec"), space)
    url, blob = IDEOGRAM_CONFIG
    body = fetch(url)
    assert hashlib.sha1(b"blob %d\0" % len(body) + body).hexdigest() == blob, "the config is not the official blob"
    config = {k: v for k, v in json.loads(body).items() if not k.startswith("_")}
    from diffusers import Ideogram4Transformer2DModel

    with torch.device("meta"):
        census = {k: tuple(v.shape) for k, v in Ideogram4Transformer2DModel(**config).state_dict().items()}
    source = {k: s for k, (_, s) in header(VECTORS / "ideogram4-headers" / "transformer__diffusion_pytorch_model.safetensors").items()
              if not k.endswith(".weight_scale")}
    stand_ins = {k: h3_index(s[0]) if len(s) > 1 else torch.arange(s[0]) for k, s in source.items()}
    origin = {id(t): k for k, t in stand_ins.items()}
    converted = space["convert_ideogram4_transformer_checkpoint_to_diffusers"](dict(stand_ins))
    mapped, expect = {}, {}
    for target, tensor in converted.items():
        key = origin.get(id(tensor)) or next(k for k, t in stand_ins.items() if tensor.untyped_storage().data_ptr()
                                              == t.untyped_storage().data_ptr())
        shape = source[key]
        ops = row_ops(tensor.reshape(tensor.shape[0], -1)[:, 0].numpy(), shape)
        if ops:
            expect[key] = list(shape)
        mapped[target] = row(target, key, ops)
        got = apply(np.zeros(shape, np.int8), ops) if ops else np.zeros(shape, np.int8)
        assert got.shape == census[target], (target, got.shape, census[target])
    assert sorted(mapped) == sorted(census), "targets are not the transformer's census"
    component = {"name": "transformer", "folders": {"transformer": "transformer",
                                                    "unconditional_transformer": "unconditional_transformer"},
                 "expect": dict(sorted(expect.items())), "rows": [mapped[k] for k in census]}
    return {"name": "ideogram4.fp8", "components": [component]}


#: Qwen3.5-architecture MoE checkpoints whose experts are stored fused, one table per key set.
QWEN_MOE = {"qwen3.6-35b-a3b.experts": "Qwen/Qwen3.6-35B-A3B@995ad96eacd98c81ed38be0c5b274b04031597b0"}
#: A fused tensor's role, and the per-expert Linear it becomes in cozy-runtime's `moe.RoutedExpert`.
QWEN_EXPERTS = {"gate_up_proj": "gate_and_up_proj", "down_proj": "down_proj"}


def qwen_moe(name: str, pinned: str) -> dict[str, Any]:
    """Every key as stored, except each layer's fused experts (`experts.gate_up_proj` [E, 2I, H],
    `experts.down_proj` [E, H, I]), which become one row per expert, `experts.{e}.gate_and_up_proj.weight`
    and `.down_proj.weight`: the blocks cozy-runtime's `moe.split` builds, which its weight paging moves one
    expert at a time. The source's config stays the construction config (`keep_config`). Targets are the
    census of Transformers' model plus the multi-token-prediction layer cozy-runtime's `lm.Draft` adds
    (`mtp.*`, one decoder layer at the first full-attention index), experts split."""
    repo, _, commit = pinned.partition("@")
    index = json.loads(fetch(f"{HF}/{repo}/resolve/{commit}/model.safetensors.index.json"))["weight_map"]
    config = json.loads(fetch(f"{HF}/{repo}/resolve/{commit}/config.json"))
    text = config["text_config"]
    experts, inner, hidden = text["num_experts"], text["moe_intermediate_size"], text["hidden_size"]
    fused = {"gate_up_proj": [experts, 2 * inner, hidden], "down_proj": [experts, hidden, inner]}
    with torch.device("meta"):
        model = AutoModelForImageTextToText.from_config(AutoConfig.for_model(**config))
        space = sys.modules[type(model).__module__]
        layer = space.Qwen3_5MoeDecoderLayer(model.config.text_config, text["layer_types"].index("full_attention"))
    census = [*model.state_dict(), "mtp.fc.weight", "mtp.pre_fc_norm_embedding.weight", "mtp.pre_fc_norm_hidden.weight",
              *(f"mtp.layers.0.{k}" for k in layer.state_dict()), "mtp.norm.weight"]
    assert set(census) == set(index), sorted(set(census) ^ set(index))[:5]
    rows, expect, targets = [], {}, []
    for key in census:
        stem, _, role = key.rpartition(".")
        if not (stem.endswith(".experts") and role in QWEN_EXPERTS):
            rows.append(row(key, key))
            targets.append(key)
            continue
        shape = fused[role]
        expect[key] = shape
        ops = {"take": [0, ["{e}"]], "shape": shape[1:]}
        rows.append([f"{stem}.{{e}}.{QWEN_EXPERTS[role]}.weight", key, ops, {"e": experts}])
        stand_in = np.arange(math.prod(shape[:2]) * 2).reshape(shape[0], shape[1], 2)
        for e in range(experts):
            got = apply(stand_in, {"take": [0, [e]], "shape": [shape[1], 2]})
            assert (got == stand_in[e]).all(), (key, e)
            targets.append(f"{stem}.{e}.{QWEN_EXPERTS[role]}.weight")
    assert len(targets) == len(set(targets)) == len(index) + len(expect) * (experts - 1)
    return {"name": name, "keep_config": True,
            "components": [{"name": "model", "expect": dict(sorted(expect.items())), "rows": rows}]}


def generate() -> dict[str, dict[str, Any]]:
    sdxl, layers = single_file("sdxl.single_file")
    anima, _ = single_file("anima.single_file")
    unet = types.SimpleNamespace(**reference_config(SINGLE_FILE["sdxl.single_file"]["reference"], "unet/config.json"))
    h3 = h3_converter()
    native, source = h3_native(h3)
    return {
        "sdxl.single_file": sdxl,
        "anima.single_file": anima,
        "h3.native": native,
        "h3.comfy": h3_native(h3, comfy=True)[0],
        "h3.lora": h3_lora(h3, source),
        "sdxl.lora": sdxl_lora(sdxl, layers, unet),
        "krea2.lora": krea2_lora(),
        "seedvr2": seedvr2(),
        "h3.latent_upscaler": h3_latent_upscaler(),
        "ideogram4.fp8": ideogram4(),
        **{name: qwen_moe(name, pinned) for name, pinned in QWEN_MOE.items()},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help="rewrite the tables")
    args = parser.parse_args()
    for package, version in PINS.items():
        if importlib.metadata.version(package) != version:
            sys.exit(f"{package} {importlib.metadata.version(package)} is not the pinned {version}")
    stale = []
    for name, table in generate().items():
        path = OUT / f"{name}.json"
        body = (json.dumps(table, separators=(",", ":")) + "\n").encode()
        if args.write:
            path.write_bytes(body)
        elif not path.is_file() or path.read_bytes() != body:
            stale.append(path.name)
        print(f"{path.name}: {len(body)} bytes {'written' if args.write else 'checked'}")
    if stale:
        sys.exit(f"stale converter tables {stale}: run scripts/converters.py --write")


if __name__ == "__main__":
    main()
