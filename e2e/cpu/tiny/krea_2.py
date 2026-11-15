"""Tiny Krea 2: the Diffusers folder Krea ships (`Krea2Transformer2DModel` with its norms in F32 as
`_keep_in_fp32_modules` stores them, Qwen3-VL as `Qwen3VLModel`, the Qwen-Image VAE in F32) at a few channels, with
Qwen's tokenizer. The text model keeps the 35 layers the DiT taps (2, 5, ..., 35), 32 wide. Raw and Turbo share
the configs and the weights; the function names the regime.

Its five community adapters (0.9.0, se-126) are laid out as their mirrors are (TensorFS's `krea2.lora`): one
`transformer` component of PEFT factors under Diffusers' names, `adapter_config.json` stating `r`, `lora_alpha` and
`target_modules`; Identity Edit's own alphas as F32 scalars, Patil's depth factors and its widened `img_in` (kept
whole as `img_in.modules_to_save`) in F32. `adapters.AdapterGraph` finds its linears on `Krea2Transformer2DModel()`
at the class defaults, Krea 2's geometry: the executor's defaults are the tiny DiT's (PATCH). Parts of the same checkpoint,
each a model of its own (xs-192): its transformer with its scheduler, its text encoder with its tokenizer, its VAE."""

from __future__ import annotations

import inspect
import json
import shutil
from pathlib import Path

import tensorfs
import torch
from diffusers import AutoencoderKLQwenImage, FlowMatchEulerDiscreteScheduler, Krea2Transformer2DModel
from transformers import Qwen3VLModel

from cell import NARROW, STEERS, Cell, Json, module, qwen_tokenizer, save, size
from tiny import _shared

RAW, TURBO = "krea/krea-2@0.1.0", "krea/krea-2-turbo@0.1.0"
EDIT, DEPTH, POSE = "conradlocke/identity-edit@0.1.0", "patil/depth-control@0.1.0", "thedeoxen/pose-control@0.1.0"
LINEART, INPAINT = "cierpliwy/lineart-edit@0.1.0", "cierpliwy/inpaint-edit@0.1.0"
TEXT_WIDTH, Z = 32, 16
#: The checkpoint's parts, each a model of that part alone (xs-192).
DIT, ENCODER, VAE = "aimeasop/redcraft-krea@0.2.0", "quiho/qwen3-vl-4b-abliterated@0.2.0", "e2e/krea-2-vae@0.1.0"
GEOMETRY: dict[str, object] = {
    "in_channels": 4 * Z, "num_layers": 2, "attention_head_dim": 16, "num_attention_heads": 2, "num_key_value_heads": 1,
    "intermediate_size": 64, "timestep_embed_dim": 32, "text_hidden_dim": TEXT_WIDTH, "num_text_layers": 12,
    "text_num_attention_heads": 2, "text_num_key_value_heads": 2, "text_intermediate_size": 64, "axes_dims_rope": (4, 6, 6),
}


def _defaults() -> tuple[object, ...]:
    """`Krea2Transformer2DModel.__init__`'s defaults (Diffusers' `register_to_config` reads and calls the function it
    wraps) with the tiny DiT's geometry."""
    init = Krea2Transformer2DModel.__init__
    assert getattr(init, "__wrapped__", None) is inspect.unwrap(init), "PATCH names the function the defaults are read from"
    parameters = list(inspect.signature(inspect.unwrap(init)).parameters.values())[1:]
    assert all(p.kind is p.POSITIONAL_OR_KEYWORD and p.default is not p.empty for p in parameters), parameters
    assert set(GEOMETRY) <= {p.name for p in parameters}, GEOMETRY
    return tuple(GEOMETRY.get(p.name, p.default) for p in parameters)


PATCH: dict[str, object] = {"krea_2.adapters:Krea2Transformer2DModel.__init__.__wrapped__.__defaults__": _defaults()}
#: Each adapted block's linears, as every mirror's `target_modules` names them.
_SITES = ("attn.to_gate", "attn.to_k", "attn.to_out.0", "attn.to_q", "attn.to_v", "ff.down", "ff.gate", "ff.up")
_DIT = tuple(f"transformer_blocks.{i}" for i in range(2))
_TEXT = tuple(f"text_fusion.{kind}_blocks.{i}" for kind in ("layerwise", "refiner") for i in range(2))
#: The factors' spread: armed, each adapter's factors move its cell's picture 2.7-3.7/255 (at 0.1, 0.03: under a
#: reference's 1/255). Patil's projection takes the tiny `img_in`'s ~0.1.
_FACTOR, _PROJECTION = 1.0, 0.1


#: The q/k norms' gain. Krea2RMSNorm scales by `1 + weight`, 1 at its init: a 16-wide head's logits are then about
#: N(0, 1) over some 2000 keys, near-uniform, so a map's tokens reached the picture only as their mean (a moved lineart
#: map moved it 0.45/255, a moved skeleton 0.03, CFG at 5 against 0 0.01). At 3 attention selects, as trained Krea's
#: does (a moved map moves it 1.2-1.9/255), and a zeroed adapter still moves it 0.05 at most (e2e 43352463205).
_QK_GAIN = 3.0


def _transformer() -> torch.nn.Module:
    transformer = module(Krea2Transformer2DModel(**GEOMETRY)).to(torch.bfloat16)
    kept = {"norm", "norm1", "norm2", "norm_q", "norm_k"}
    for name, child in transformer.named_modules():
        leaf = name.rsplit(".", 1)[-1]
        if leaf in kept:
            child.to(torch.float32)
        weight = getattr(child, "weight", None)
        if leaf in ("norm_q", "norm_k") and isinstance(weight, torch.Tensor):
            with torch.no_grad():
                weight.fill_(_QK_GAIN - 1)
    return transformer


def _adapter(store: Path, seed: int, rank: int, blocks: tuple[str, ...], dtype: torch.dtype = torch.bfloat16, *,
             alphas: bool = False, projection: bool = False) -> str:
    """One mirror at `rank` (each states `lora_alpha` = `r`) over the tiny DiT's linears in `blocks`: its stated alphas
    (`alphas`), its `[hidden, 2 * in_channels]` input projection (`projection`)."""
    with torch.device("meta"):
        dit = module(Krea2Transformer2DModel(**GEOMETRY))
    sites = sorted(f"{block}.{site}" for block in blocks for site in _SITES)
    targets: list[Json] = list(sites)
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("transformer", cls=None, config_file="adapter_config.json",
                                 config={"lora_alpha": rank, "peft_type": "LORA", "r": rank, "target_modules": targets})
    generator = torch.Generator().manual_seed(seed)
    names = {torch.bfloat16: "BF16", torch.float32: "F32"}

    def put(key: str, data: torch.Tensor, shape: list[int] | None = None) -> None:
        data = data.contiguous()
        component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=names[data.dtype],
                      shape=list(data.shape) if shape is None else shape)

    def drawn(*shape: int, spread: float = _FACTOR) -> torch.Tensor:
        return (torch.randn(shape, generator=generator) * spread).to(dtype)

    for site in sites:
        linear = dit.get_submodule(site)
        assert isinstance(linear, torch.nn.Linear), site
        put(f"{site}.lora_A.weight", drawn(rank, linear.in_features))
        put(f"{site}.lora_B.weight", drawn(linear.out_features, rank))
        if alphas:
            put(f"{site}.alpha", torch.tensor([float(rank)]), [])  # a scalar's bytes (a 0-d tensor has no byte view)
    if projection:
        image = dit.get_submodule("img_in")
        assert isinstance(image, torch.nn.Linear)
        put("img_in.modules_to_save.weight", drawn(image.out_features, 2 * image.in_features, spread=_PROJECTION))
        put("img_in.modules_to_save.bias", drawn(image.out_features, spread=_PROJECTION))
    return writer.commit().id


def _part(pipeline: Path, out: Path, names: tuple[str, ...]) -> None:
    """Some of `pipeline`'s components, as a pipeline of only those: the same files, so the same weights."""
    index = json.loads((pipeline / "model_index.json").read_text())
    for name in names:
        shutil.copytree(pipeline / name, out / name)
    (out / "model_index.json").write_text(json.dumps({key: index[key] for key in ("_class_name", "_diffusers_version", *names)}))


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "krea-2"
    torch.manual_seed(0)
    qwen_tokenizer(out / "tokenizer")
    save(module(Qwen3VLModel(_shared.qwen3_vl(35, TEXT_WIDTH))).to(torch.bfloat16), out / "text_encoder")
    save(_transformer(), out / "transformer")
    save(module(AutoencoderKLQwenImage(base_dim=8, z_dim=Z, num_res_blocks=1)), out / "vae")
    save(FlowMatchEulerDiscreteScheduler(use_dynamic_shifting=True, base_shift=0.5, max_shift=1.15, base_image_seq_len=256,
                                         max_image_seq_len=6400, shift_terminal=None), out / "scheduler")
    (out / "model_index.json").write_text(json.dumps({
        "_class_name": "Krea2Pipeline", "_diffusers_version": "0.42.1", "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
        "text_encoder": ["transformers", "Qwen3VLModel"], "tokenizer": ["transformers", "Qwen2TokenizerFast"],
        "transformer": ["diffusers", "Krea2Transformer2DModel"], "vae": ["diffusers", "AutoencoderKLQwenImage"]}))
    # Each mirror's rank over 32 (256, 64, 32, 64, 64).
    _part(out, root / "dit", ("transformer", "scheduler"))
    _part(out, root / "text-encoder", ("text_encoder", "tokenizer"))
    _part(out, root / "vae", ("vae",))
    return {RAW: out, TURBO: out, DIT: root / "dit", ENCODER: root / "text-encoder", VAE: root / "vae",
            EDIT: _adapter(store, 1, 8, _TEXT + _DIT, alphas=True),
            DEPTH: _adapter(store, 2, 2, _DIT, torch.float32, projection=True), POSE: _adapter(store, 3, 1, _TEXT + _DIT),
            LINEART: _adapter(store, 4, 2, _TEXT + _DIT), INPAINT: _adapter(store, 5, 2, _TEXT + _DIT)}


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_FOX = _shared.picture("fox", 640, 512)
#: Square, so the box's edit makes the 1 MP canvas its text-only twin (image/turbo) makes.
_SQUARE = _shared.picture("square", 512, 512)
#: 512², Krea's smallest canvas (0.25 MP).
_SMALL: dict[str, Json] = {"prompt": _prompt("a red fox in fresh snow"), "seed": 7, "megapixels": 0.25, "steps": 2}

CELLS: list[Cell] = [
    {"entry": "generate", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "generate", "name": "negative", "payload": {**_SMALL, "negative_prompt": "blurry", "guidance": 5.0}},
    {"entry": "generate", "name": "no-cfg", "payload": {**_SMALL, "guidance": 0.0}},
    {"entry": "generate", "name": "wide", "payload": {**_SMALL, "aspect_ratio": "16:9"}, "check": size(688, 384)},
    {"entry": "generate_turbo", "name": "text", "payload": dict(_SMALL), "check": size(512, 512)},
    {"entry": "image", "name": "turbo", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "megapixels": 1},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "raw", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "megapixels": 1,
                                                  "turbo": False}, "check": size(1024, 1024)},
    # Each slot bound to a model of its part alone: the same weights as `image/turbo`'s, composed at run time (xs-192).
    {"entry": "image", "name": "parts", "payload": {"instructions": _prompt("a red fox in fresh snow"), "seed": 7, "megapixels": 1},
     "models": {"image.models.model": DIT, "image.models.text_encoder": ENCODER, "image.models.vae": VAE},
     "check": size(1024, 1024)},
    # Identity Edit on Raw with CFG: the picture's tokens before the target on both branches, Qwen3-VL reading it.
    {"entry": "generate", "name": "edit", "payload": {**_SMALL, "prompt": _prompt("the fox in <fox> wears a hat", "fox")},
     "files": {"prompt.references.0.file": _FOX}, "check": size(576, 464)},
    # Patil's depth on Turbo: generate_turbo/text's request with the map read beside each image token.
    {"entry": "generate_turbo", "name": "depth", "payload": {**_SMALL, "control": [{"kind": "depth"}]},
     "files": {"control.0.image": _shared.picture("depth", 512, 512)}, "check": size(512, 512)},
    # Each control on Raw, generate/text's request with its map after the target at time 0, attending to itself and read
    # by Qwen3-VL as `Picture 1:`: it steers away from generate/text, and another map steers it again (`apart`), but for
    # the pose's: a skeleton moved a fifth of the width moves it 0.82/255, its thin limbs being few tokens.
    {"entry": "generate", "name": "lineart", "payload": {**_SMALL, "control": [{"kind": "lineart", "strength": 0.8}]},
     "files": {"control.0.image": _shared.lines("lines", 512, 512)}, "check": size(512, 512),
     "apart": {"generate/text": STEERS}},
    {"entry": "generate", "name": "lineart-moved", "payload": {**_SMALL, "control": [{"kind": "lineart", "strength": 0.8}]},
     "files": {"control.0.image": _shared.lines("lines-moved", 512, 512, offset=48)}, "check": size(512, 512),
     "apart": {"generate/lineart": STEERS}},
    {"entry": "generate", "name": "pose", "payload": {**_SMALL, "control": [{"kind": "pose"}]},
     "files": {"control.0.image": _shared.bones("bones", 512, 512)}, "check": size(512, 512),
     "apart": {"generate/text": STEERS}},
    # The inpaint map's transparent area as black, the LoRA redrawing it.
    {"entry": "generate", "name": "inpaint", "payload": {**_SMALL, "control": [{"kind": "inpaint"}]},
     "files": {"control.0.image": _shared.holed("hole", 512, 512)}, "check": size(512, 512),
     "apart": {"generate/text": STEERS}},
    {"entry": "generate", "name": "inpaint-moved", "payload": {**_SMALL, "control": [{"kind": "inpaint"}]},
     "files": {"control.0.image": _shared.holed("hole-moved", 512, 512, centre=(0.3, 0.7))}, "check": size(512, 512),
     "apart": {"generate/inpaint": STEERS}},
    # The box's edit on Turbo: image/turbo's request with the picture it mentions (its words read the same). Another
    # picture moves it only 0.24/255 (e2e 43352463205), so it is judged against its twin alone.
    {"entry": "image", "name": "edit", "payload": {"instructions": _prompt("a red <fox> in fresh snow", "fox"), "seed": 7,
                                                   "megapixels": 1},
     "files": {"instructions.references.0.file": _SQUARE}, "check": size(1024, 1024), "apart": {"image/turbo": STEERS}},
    {"entry": "quantize", "name": "lanes", "payload": {}, "skip": NARROW},
]

