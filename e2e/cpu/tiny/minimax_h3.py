"""Tiny MiniMax H3: the package's own constructors (the AdaLN-pruned dual DiT, the truncated Qwen3-VL conditioner, the
video and audio VAEs, the PDD turbo overlay) at a few channels, with timestep plans of 2 and 3 evaluations, written as the
five-root checkpoint and the turbo LoRA by TensorFS's own writer. Beside it, a bf16-full lane (FULL DiTs: the time
embedder and AdaLN projections) and the pruned lane minimax-h3-tools makes from it.

The release geometry (56 heads x 128, 50 layers, a 5120-wide conditioner) is pinned by `_validate_model_contract`;
a tiny model cannot meet it, so that one check is waived (WAIVE) and stays GPU-only. The canvas (768 short edge) and the
clip envelope are the real ones: Diffusers' pipeline fixes them, not the checkpoint."""

from __future__ import annotations

import json
import math
from collections.abc import Callable
from importlib.resources import files
from pathlib import Path
from typing import Protocol, cast

import numpy as np
import tensorfs
import torch
from cozy_runtime.author import Config
from diffusers import AutoencoderKLMiniMaxH3Audio, MiniMaxH3Scheduler
from h3_tables import regenerate as tools_regenerate
from h3_tables.kernel import H3Topology, precompute_tables
from h3_tables.plans import parse_plan
from minimax_h3 import latent_upscaler, official
from minimax_h3.conditioner import build_text_conditioner
from minimax_h3.latent_upscaler.architecture import LatentResizer3D
from minimax_h3.official import Schedule, TimestepPlan, Trunk
from minimax_h3.vae_tiles import TileBatchedVideoVAE
from PIL import Image

from cell import Cell, Json, Source, module

WAIVE = {"minimax_h3.official:_validate_model_contract": "the release geometry: 56x128 heads, 50 layers, 5120 text width"}
REGENERATE, UPSCALER = "efficient-large-model/open-h3-regenerate-2k@0.1.0", "lbh-123-ai/h3-latent-upscaler@0.1.0"
#: The release's bf16-full lane and the bf16 lane minimax-h3-tools derives from it (`_from_full`).
FULL, FROM_FULL = "minimaxai/minimax-h3@1.0.0-rc.7:bf16-full", "minimaxai/minimax-h3@1.0.0-rc.7:bf16-from-full"
#: LatentResizer3D's defaults (in_channels, in_blocks, out_blocks, channels, dropout, temporal_every, temporal_kernel):
#: the release's 12 + 12 blocks of 512 channels (345M) shrunk to 1 + 1 of 32.
_UPSCALER_DEFAULTS = (24, 1, 1, 32, 0.1, 2, 5)
PATCH: dict[str, object] = {"minimax_h3.latent_upscaler.architecture:LatentResizer3D.__init__.__defaults__": _UPSCALER_DEFAULTS}

_TRUNKS: tuple[Trunk, Trunk] = ("fl2va", "ref2va")
_DIT = {"fl2va": "transformer", "ref2va": "transformer_ref"}
_TURBO = {"fl2va": "fl2va_turbo", "ref2va": "ref2va_turbo"}
#: Shifts whose turbo grids meet only at the origin (`TurboSchedule`).
_VIDEO_SHIFT, _AUDIO_SHIFT = 5.0, 7.0

#: Both DiTs: two heads (one per rank of a two-rank group), 24 of 32 head channels rotated (3 axes x 4 x 2).
DIT: dict[str, Json] = {
    "attention_head_dim": 32, "audio_in_channels": 32, "ffn_dim": 128, "final_norm_eps": 1e-05, "freq_dim": 32,
    "hidden_size": 64, "in_channels": 24, "norm_eps": 1e-05, "num_attention_heads": 2, "num_layers": 2,
    "num_refiner_layers": 2, "patch_size": [1, 2, 2], "qk_norm_eps": 1e-05, "rope_freq_dim": 4, "rope_theta": 10000,
    "text_dim": 64, "time_embed_dim": 32, "time_embed_hidden_dim": 64,
}


#: The tiny DiT's modulation shape, for the producer's table kernel (`H3Topology.from_config` holds the release's).
_TOPOLOGY = H3Topology(hidden_size=64, num_layers=2, freq_dim=32, time_embed_hidden_dim=64, time_embed_dim=32)


def _real() -> dict[str, dict[str, Json]]:
    """The release's own five sections (minimax-h3-tools' `model-config.json`), shrunk below."""
    document: dict[str, dict[str, Json]] = json.loads(files("h3_tables").joinpath("assets/model-config.json").read_text())
    return document


def _sections() -> dict[str, dict[str, Json]]:
    real = _real()
    text = dict(real["text_encoder"])
    text["text_config"] = {**_mapping(text["text_config"]), "hidden_size": 64, "intermediate_size": 128,
                           "num_attention_heads": 4, "num_key_value_heads": 2, "head_dim": 16,
                           "rope_scaling": {"mrope_interleaved": True, "mrope_section": [4, 2, 2], "rope_type": "default"}}
    text["vision_config"] = {**_mapping(text["vision_config"]), "depth": 3, "hidden_size": 32, "intermediate_size": 64,
                             "num_heads": 2, "out_hidden_size": 64, "deepstack_visual_indexes": [0, 1, 2]}
    vae: dict[str, Json] = {**real["vae"], "block_out_channels": [8, 8, 8, 16, 16, 16], "norm_num_groups": 4, "layers_per_block": 1,
           "decoder_attention_head_dim": 16, "decoder_num_attention_heads": 2, "decoder_num_layers": 1}
    audio: dict[str, Json] = {**real["audio_vae"], "encoder_dim": 8, "latent_dim": 64, "num_attention_heads": 2, "decoder_dim": 128}
    return {"text_encoder": text, "vae": vae, "audio_vae": audio}


def _mapping(value: Json) -> dict[str, Json]:
    assert isinstance(value, dict)
    return value


class _Scheduler(Protocol):
    """Diffusers' scheduler, whose decorated constructor is untyped."""

    sigmas: torch.Tensor

    def set_timesteps(self, num_inference_steps: int) -> None: ...


def _schedule(evaluations: int) -> Schedule:
    def sigmas(shift: float) -> tuple[float, ...]:
        scheduler = cast(_Scheduler, MiniMaxH3Scheduler(shift=shift))
        scheduler.set_timesteps(evaluations + 1)
        values = tuple(float(v) for v in scheduler.sigmas.tolist())
        assert len(values) == evaluations + 1
        return values

    return Schedule(sigma_grid_points=evaluations + 1, video_sigmas=sigmas(_VIDEO_SHIFT), audio_sigmas=sigmas(_AUDIO_SHIFT))


def _plan(trunk: Trunk, evaluations: tuple[int, ...]) -> TimestepPlan:
    return TimestepPlan(task=trunk, video_shift=_VIDEO_SHIFT, audio_shift=_AUDIO_SHIFT,
                        schedules=tuple(_schedule(n) for n in evaluations))


def _keys(plan: TimestepPlan) -> Json:
    document: dict[str, Json] = json.loads(plan.canonical_bytes())
    return document["table_keys"]


def _fill(root: torch.nn.Module, seed: int) -> None:
    """Every matrix drawn from one seed (a table or factor small: they are modulation rows and low-rank deltas); vectors
    keep their constructed values. `torch.empty` leaves the rest undefined, and a reference needs every byte fixed."""
    generator = torch.Generator().manual_seed(seed)
    with torch.no_grad():
        for name, parameter in root.named_parameters():
            if parameter.dim() < 2:
                continue
            fan_in = math.prod(parameter.shape[1:])
            std = 0.02 if name.endswith(("table", "lora_up", "lora_down")) else fan_in**-0.5
            value = torch.randn(parameter.shape, generator=generator, dtype=torch.float32) * std
            parameter.copy_(value.to(parameter.dtype))
        for name, tensor in root.state_dict().items():
            if tensor.is_floating_point() and not bool(torch.isfinite(tensor).all()):
                raise ValueError(f"{name} is not finite after filling")


def _write(writer: tensorfs.Writer, name: str, root: torch.nn.Module, config: dict[str, Json], assets: dict[str, bytes]) -> None:
    names = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32", torch.int64: "I64"}
    component = writer.component(name, cls=None, config=config)
    for key, tensor in root.state_dict().items():
        data = tensor.detach().contiguous()
        component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=names[data.dtype],
                      shape=list(data.shape))
    for asset, data in assets.items():
        component.asset(asset, data)


def models(root: Path, store: Path) -> dict[str, Path | str]:
    torch.manual_seed(0)
    sections = _sections()
    plans = {trunk: _plan(trunk, (2, 3)) for trunk in _TRUNKS}
    turbo = {trunk: _plan(trunk, (8,)) for trunk in _TRUNKS}
    base = tensorfs.Writer(cache_dir=store)
    text = module(build_text_conditioner(sections["text_encoder"]))
    _fill(text, 1)
    _write(base, "text_encoder", text, sections["text_encoder"], {})
    vae = module(official._apply_video_vae_dtype(TileBatchedVideoVAE.from_config(sections["vae"])))
    _fill(vae, 2)
    _write(base, "vae", vae, sections["vae"], {})
    audio = module(AutoencoderKLMiniMaxH3Audio.from_config(sections["audio_vae"]).eval())
    _fill(audio, 3)
    _write(base, "audio_vae", audio, sections["audio_vae"], {})
    for seed, trunk in enumerate(_TRUNKS, 4):
        config = {**DIT, "cozy_h3": {"task": trunk, "modulation": "adaln-pruned", "table_keys": _keys(plans[trunk])}}
        spec = official._dit_spec(Config({_DIT[trunk]: config}), trunk)
        dit = module(official._build_dit(*spec))
        _fill(dit, seed)
        _write(base, _DIT[trunk], dit, config, {official.PLAN_ASSET: plans[trunk].canonical_bytes()})
    lora = tensorfs.Writer(cache_dir=store)
    overlays = {
        _TURBO[trunk]: {**DIT, "cozy_h3": {"task": trunk, "modulation": "adaln-pruned", "distillation": "pdd",
                                           "lora_rank": 64, "lora_alpha": 64.0, "pdd_num_steps": 32, "pdd_block_size": 4,
                                           "table_keys": _keys(turbo[trunk])}}
        for trunk in _TRUNKS
    }
    built = official.OfficialH3TurboLoRA(Config(overlays), turbo)
    for seed, trunk in enumerate(_TRUNKS, 6):
        overlay = module(built.components[_TURBO[trunk]])
        _fill(overlay, seed)
        _write(lora, _TURBO[trunk], overlay, overlays[_TURBO[trunk]], {official.PLAN_ASSET: turbo[trunk].canonical_bytes()})
    built_models: dict[str, Path | str] = {
        "minimaxai/minimax-h3@1.0.0-rc.7": base.commit().id, "alibaba-pai/minimax-h3-turbo-lora@1.0.1": lora.commit().id,
        REGENERATE: _regenerate(store), UPSCALER: _upscaler(store)}
    shared = {"text_encoder": (text, sections["text_encoder"]), "vae": (vae, sections["vae"]),
              "audio_vae": (audio, sections["audio_vae"])}
    built_models[FULL], built_models[FROM_FULL] = _from_full(store, plans, shared)
    return built_models


def _from_full(store: Path, plans: dict[Trunk, TimestepPlan],
               shared: dict[str, tuple[torch.nn.Module, dict[str, Json]]]) -> tuple[str, str]:
    """A bf16-full lane (both DiTs FULL, every matrix drawn) and the bf16 lane minimax-h3-tools derives from it: each DiT's
    tables computed by the producer's own kernel (`precompute_tables`) from the FULL DiT's time embedder and AdaLN
    projections, every other row the same. Its own random stream, so the models above keep theirs."""
    full, pruned = tensorfs.Writer(cache_dir=store), tensorfs.Writer(cache_dir=store)
    for name, (root, config) in shared.items():
        _write(full, name, root, config, {})
        _write(pruned, name, root, config, {})
    with torch.random.fork_rng():
        torch.manual_seed(10)
        for seed, trunk in enumerate(_TRUNKS, 10):
            assets = {official.PLAN_ASSET: plans[trunk].canonical_bytes()}
            config = {**DIT, "cozy_h3": {"task": trunk, "modulation": "full"}}
            dit = module(official._build_dit(*official._dit_spec(Config({_DIT[trunk]: config}), trunk)))
            _fill(dit, seed)
            _write(full, _DIT[trunk], dit, config, assets)
            state = dit.state_dict()
            tables: dict[str, torch.Tensor] = {}
            precompute_tables(plan=parse_plan(assets[official.PLAN_ASSET], task=trunk), topology=_TOPOLOGY,
                              read=lambda key, dtype, shape: state[key], write=tables.__setitem__,
                              progress=lambda done, total: None, device=torch.device("cpu"))
            config = {**DIT, "cozy_h3": {"task": trunk, "modulation": "adaln-pruned", "table_keys": _keys(plans[trunk])}}
            twin = module(official._build_dit(*official._dit_spec(Config({_DIT[trunk]: config}), trunk)))
            twin.load_state_dict({**{key: state[key] for key in twin.state_dict() if key in state}, **tables})
            _write(pruned, _DIT[trunk], twin, config, assets)
    return full.commit().id, pruned.commit().id


def _regenerate(store: Path) -> str:
    """Open-H3-Regenerate-2K's rank-576 refine overlay over the tiny FL2VA DiT, with the refine plan the tools write
    (`h3_tables.regenerate.plan`: its three starts)."""
    plan = official.timestep_plan(tools_regenerate.plan().canonical_bytes, "fl2va")
    config: dict[str, Json] = {**DIT, "cozy_h3": {"task": "fl2va", "modulation": "adaln-pruned", "distillation": "regenerate",
                                                  "lora_rank": 576, "lora_alpha": 576.0, "table_keys": _keys(plan)}}
    overlay = module(official.OfficialH3RegenerateLoRA(Config(config), plan).components[official.REGENERATE_COMPONENT])
    _fill(overlay, 8)
    writer = tensorfs.Writer(cache_dir=store)
    _write(writer, official.REGENERATE_COMPONENT, overlay, config, {official.PLAN_ASSET: plan.canonical_bytes()})
    return writer.commit().id


def _upscaler(store: Path) -> str:
    """LBH-123-AI's 3D-conv latent upscaler at the PATCHed width (one block each side, 32 channels)."""
    upscaler = module(LatentResizer3D(*_UPSCALER_DEFAULTS)).bfloat16()
    _fill(upscaler, 9)
    writer = tensorfs.Writer(cache_dir=store)
    _write(writer, "upscaler", upscaler, {"architecture": latent_upscaler.ARCHITECTURE}, {})
    return writer.commit().id


def picture(name: str, width: int, height: int) -> Callable[[Path], Path]:
    """A smooth picture with structure (a gradient and a disc), made where the cell runs."""

    def make(folder: Path) -> Path:
        y, x = np.mgrid[0:height, 0:width].astype(np.float32)
        disc = ((x - width * 0.6) ** 2 + (y - height * 0.45) ** 2) < (min(width, height) * 0.25) ** 2
        rgb = np.stack([x / width * 255, y / height * 255, np.where(disc, 230.0, 40.0)], axis=-1).astype(np.uint8)
        path = folder / f"{name}.png"
        Image.fromarray(rgb).save(path)
        return path

    return make


#: A shot's only segment carries nothing on: its context and latents are empty by design.
_EMPTY = ["context", "latents"]
_SEGMENT: dict[str, Json] = {"prompt": "a red fox runs across a snowy field", "seed": 7, "duration_s": 5, "steps": 2,
                             "turbo": False, "canvas": [1344, 768]}
_REF2VA: dict[str, Json] = {**_SEGMENT, "frames": 124, "videos": []}
_REGENERATE: dict[str, Json] = {"prompt": "a red fox runs across a snowy field", "seed": 7, "resolution": "768p",
                                "quality": "full"}


def clip(frames: int) -> Callable[[Path], Path]:
    """A second of 320x180 video at 24 fps with a tone, each frame a shifted gradient and a disc."""

    def make(folder: Path) -> Path:
        import av

        path = folder / f"shot-{frames}.mp4"
        with av.open(str(path), "w") as container:
            video = container.add_stream("libx264", rate=24)
            video.width, video.height, video.pix_fmt = 320, 180, "yuv420p"
            sound = container.add_stream("aac", rate=48000)
            y, x = np.mgrid[0:180, 0:320]
            for index in range(frames):
                disc = ((x - 100 - index * 4) ** 2 + (y - 90) ** 2) < 900
                rgb = np.stack([(x + index * 6) % 256, y, np.where(disc, 230, 40)], axis=-1).astype(np.uint8)
                for packet in video.encode(av.VideoFrame.from_ndarray(rgb, format="rgb24")):
                    container.mux(packet)
            for packet in video.encode():
                container.mux(packet)
            tone = (np.sin(np.arange(48000 * frames // 24) * 0.03) * 0.3).astype(np.float32)[None]
            chunk = av.AudioFrame.from_ndarray(tone, format="fltp", layout="mono")
            chunk.sample_rate = 48000
            for packet in [*sound.encode(chunk), *sound.encode()]:
                container.mux(packet)
        return path

    return make


#: A base (untabled) step on the CPU: the executor computes in float32 (Runtime `weights._float32`), so FULL's AdaLN rows
#: skip the bf16 rounding the stored tables took (on a bf16 GPU both round): 2% of pixels one level apart.
_UNROUNDED = 0.25


def _full_lane() -> list[Cell]:
    """bf16-full beside the pruned lane made from it (se-111), on each path: FULL computes at every step the rows the
    pruned lane stores, and an overlay's tables (turbo, Regenerate) replace either's, so those pairs are equal to the bit."""
    turbo: dict[str, Json] = {"steps": None, "turbo": True}
    reference: dict[str, Source] = {"assets.0.asset": picture("fox", 512, 512)}
    paths: list[tuple[str, str, dict[str, Json], dict[str, Source], float]] = [
        ("fl2va_segment", "text", {"payload": _SEGMENT}, {}, _UNROUNDED),
        ("fl2va_segment", "turbo", {"payload": {**_SEGMENT, **turbo}}, {}, 0.0),
        ("ref2va_segment", "reference", {"payload": _REF2VA, "assets": [{"label": "fox"}]}, reference, _UNROUNDED),
        ("ref2va_segment", "turbo", {"payload": {**_REF2VA, **turbo}, "assets": [{"label": "fox"}]}, reference, 0.0),
        ("regenerate_shot", "native", {"payload": _REGENERATE}, {"payload.video": clip(25)}, 0.0),
    ]
    cells: list[Cell] = []
    for entry, name, payload, files, tolerance in paths:
        twin: Cell = {"entry": entry, "name": f"from-full-{name}", "payload": payload, "files": files,
                      "models": {f"{entry}.models.base_model": FROM_FULL}}
        if entry != "regenerate_shot":
            twin["empty"] = _EMPTY
        full: Cell = {**twin, "name": f"full-{name}", "models": {f"{entry}.models.base_model": FULL},
                      "same_as": f"{entry}/from-full-{name}", "tolerance": tolerance}
        cells += [twin, full]
    # FULL across a two-rank group: Diffusers' plan splits its live modulation's rows as it splits the tables'.
    cells.append({"entry": "fl2va_segment", "name": "full-turbo-2-ranks", "payload": {"payload": {**_SEGMENT, **turbo}},
                  "models": {"fl2va_segment.models.base_model": FULL}, "ranks": 2, "empty": _EMPTY,
                  "same_as": "fl2va_segment/full-turbo"})
    return cells


CELLS: list[Cell] = [
    {"entry": "fl2va_segment", "name": "text", "payload": {"payload": _SEGMENT}, "empty": _EMPTY},
    {"entry": "fl2va_segment", "name": "turbo", "payload": {"payload": {**_SEGMENT, "steps": None, "turbo": True}}, "empty": _EMPTY},
    {"entry": "fl2va_segment", "name": "first-frame", "payload": {"payload": _SEGMENT},
     "files": {"payload.first_frame": picture("first", 1344, 768)}, "empty": _EMPTY},
    {"entry": "ref2va_segment", "name": "reference", "payload": {"payload": _REF2VA, "assets": [{"label": "fox"}]},
     "files": {"assets.0.asset": picture("fox", 512, 512)}, "empty": _EMPTY},
    {"entry": "fl2va_segment", "name": "text-2-ranks", "payload": {"payload": _SEGMENT}, "ranks": 2, "empty": _EMPTY,
     "same_as": "fl2va_segment/text"},
    {"entry": "fl2va_segment", "name": "turbo-2-ranks", "payload": {"payload": {**_SEGMENT, "steps": None, "turbo": True}},
     "ranks": 2, "empty": _EMPTY, "same_as": "fl2va_segment/turbo"},
    {"entry": "ref2va_segment", "name": "reference-2-ranks", "payload": {"payload": _REF2VA, "assets": [{"label": "fox"}]},
     "files": {"assets.0.asset": picture("fox", 512, 512)}, "ranks": 2, "empty": _EMPTY,
     "same_as": "ref2va_segment/reference"},
    # The orchestrators: their segments run as child calls through the driver's broker (TensorD's `calls.rs`).
    {"entry": "generate", "name": "beat", "payload": {
        "beats": [{"duration": 5, "description": {"text": "a red fox runs across a snowy field"}}], "seed": 7,
        "quality": "full"}},
    {"entry": "upscale", "name": "1080p", "payload": {"video": {"description": "a red fox in the snow"}, "to": "1080p",
                                                      "strength": 0.7},
     "files": {"video.file": clip(25)}},
    {"entry": "edit", "name": "plate", "payload": {},
     "skip": "an edit calls its segment with an edit LoRA as a model choice, which TensorD binds from the Hub"},
    {"entry": "regenerate_shot", "name": "native", "payload": {"payload": _REGENERATE},
     "files": {"payload.video": clip(25)}},
    # The refine overlay on every rank (5ac8a48e armed it on rank 0 alone).
    {"entry": "regenerate_shot", "name": "native-2-ranks", "payload": {"payload": _REGENERATE},
     "files": {"payload.video": clip(25)}, "ranks": 2, "same_as": "regenerate_shot/native"},
    {"entry": "regenerate_shot", "name": "1080p", "payload": {"payload": {**_REGENERATE, "resolution": "1080p",
                                                                          "strength": 0.7}},
     "files": {"payload.video": clip(25)}},
    *_full_lane(),
]
