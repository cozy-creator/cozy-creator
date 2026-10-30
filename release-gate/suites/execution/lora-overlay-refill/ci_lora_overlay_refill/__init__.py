"""Qwen-Image-Edit-2511's DiT (its int8 lane) and its Lightning LoRA, the LoRA armed by the DiT's
own forward pre-hook from that forward's `attention_kwargs`, as H3 arms its Turbo overlay
(`adaln_pruned._arm`). It denoises with no reference image, as a text-to-image DiT.

Qwen-Image's own package arms before its DiT's component scope opens; H3 arms inside the forward,
after the scope (and any scope hold) has bound blocks. This cell serves the real checkpoints through
the same Runtime path H3 Turbo takes: `lora.Hook` on every LoRA'd linear, `_Bakes` baking the
armed factors into the int8 lane's resident pages on fill, and residency reusing or refilling those
pages while a ballast keeps the DiT paging within and between calls (`exercise`)."""

from __future__ import annotations

import json
from functools import partial
from itertools import pairwise
from typing import Any

import msgspec
import torch
from cozy_runtime import lora, rng
from cozy_runtime.author import App, Config, Context, Loader, Model, Telemetry, uses_components
from diffusers import QwenImageTransformer2DModel
from torch import nn

app = App()

#: The forward's own option that carries the armed overlay (H3: `OVERLAY_KWARG`).
OVERLAY = "ci_overlay"
#: The DiT linears the Lightning LoRA adapts in every block (all twelve).
SITES = (
    "attn.to_q",
    "attn.to_k",
    "attn.to_v",
    "attn.to_out.0",
    "attn.add_q_proj",
    "attn.add_k_proj",
    "attn.add_v_proj",
    "attn.to_add_out",
    "img_mlp.net.0.proj",
    "img_mlp.net.2",
    "txt_mlp.net.0.proj",
    "txt_mlp.net.2",
)
WIDTH, HIDDEN, BLOCKS, RANK = 3072, 12288, 60, 64
LORA_COMPONENT = "model"
#: The LoRA checkpoints' stored dtypes (kohya's `alpha` is i64), as Qwen-Image's package maps them.
_DTYPES = {"f32": torch.float32, "bf16": torch.bfloat16, "f16": torch.float16, "i64": torch.int64}


def _armed(dit: nn.Module, path: str) -> nn.Module | None:
    arming = dit._ci_arming
    return None if arming is None else arming.get_submodule(path)


class DiTGraph:
    """The DiT in bf16 with a permanent `lora.Hook` per site, armed per forward by its pre-hook."""

    def __init__(self, config: Config) -> None:
        dit = QwenImageTransformer2DModel.from_config(config.section("transformer").mapping())
        dit = dit.to(dtype=torch.bfloat16)
        dit._ci_arming = None

        def arm(_: nn.Module, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
            options = dict(kwargs.get("attention_kwargs") or {})
            dit._ci_arming = options.pop(OVERLAY, None)
            return args, {**kwargs, "attention_kwargs": options}

        def disarm(*_: object) -> None:
            dit._ci_arming = None

        dit.register_forward_pre_hook(arm, with_kwargs=True)
        dit.register_forward_hook(disarm, always_call=True)
        for index in range(len(dit.transformer_blocks)):
            for site in SITES:
                path = f"transformer_blocks.{index}.{site}"
                hook = lora.Hook(partial(_armed, dit, path))
                dit.get_submodule(path).register_forward_hook(hook)
        self.components: dict[str, nn.Module] = {"transformer": dit}


class Factors(nn.Module):
    """One linear's kohya factors (`lora_A`/`lora_B`) and alpha, as Qwen-Image's package reads."""

    def __init__(self, inputs: int, outputs: int, weights: torch.dtype, alpha: torch.dtype) -> None:
        super().__init__()
        self.lora_A = nn.Linear(inputs, RANK, bias=False, dtype=weights)
        self.lora_B = nn.Linear(RANK, outputs, bias=False, dtype=weights)
        self.register_buffer("alpha", torch.zeros((), dtype=alpha))
        self._scale: float | None = None

    @property
    def scale(self) -> float:
        if self._scale is None:
            self._scale = float(self.alpha) / self.lora_A.weight.shape[0]
        return self._scale

    def forward(self, x: torch.Tensor, out: torch.Tensor) -> None:
        lora.accumulate(x, out, self.lora_A.weight, self.lora_B.weight, self.scale)

    def factors(self) -> tuple[torch.Tensor, torch.Tensor, float]:
        return self.lora_A.weight, self.lora_B.weight, self.scale


class LoRAGraph:
    def __init__(self, config: Config) -> None:
        def dtype(name: str) -> torch.dtype:
            key = f"{LORA_COMPONENT}.transformer_blocks.0.{SITES[0]}.{name}"
            return _DTYPES[config.tensor_dtype(key, default="f32")]

        weights, alpha = dtype("lora_A.weight"), dtype("alpha")
        root = nn.Module()
        root.transformer_blocks = nn.ModuleList()
        for _ in range(BLOCKS):
            block = nn.Module()
            for site in SITES:
                parent = block
                *path, leaf = site.split(".")
                for name in path:
                    if not hasattr(parent, name):
                        parent.add_module(name, nn.Module())
                    parent = getattr(parent, name)
                sizes = (
                    (WIDTH, HIDDEN)
                    if site.endswith("net.0.proj")
                    else (HIDDEN, WIDTH)
                    if site.endswith("net.2")
                    else (WIDTH, WIDTH)
                )
                parent.add_module(leaf, Factors(*sizes, weights, alpha))
            root.transformer_blocks.append(block)
        self.components: dict[str, nn.Module] = {LORA_COMPONENT: root}


class Inputs(msgspec.Struct, frozen=True):
    seed: int
    size: int
    steps: int


class QwenDiT(Model[DiTGraph], encoded_leaves="accept"):
    graph: DiTGraph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(DiTGraph, factory=DiTGraph)

    @uses_components("transformer")
    def denoise(self, inputs: Inputs, overlay: nn.Module | None, after: Any) -> torch.Tensor:
        """`inputs.steps` Euler steps from seeded noise and seeded text rows, each DiT forward
        armed with `overlay` (or a base forward); `after(step)` runs after each forward."""
        dit = self.graph.components["transformer"]
        device = torch.device(str(self.device))
        side = inputs.size // 16
        latents = rng.noise((1, side * side, 64), inputs.seed, dtype=torch.bfloat16).to(device)
        text = rng.noise((1, 64, 3584), inputs.seed + 1, dtype=torch.bfloat16).to(device) * 0.1
        sigmas = torch.linspace(1.0, 1.0 / (inputs.steps + 1), inputs.steps + 1).tolist()
        with torch.inference_mode():
            for step, (sigma, following) in enumerate(pairwise(sigmas)):
                options = {} if overlay is None else {OVERLAY: overlay}
                velocity = dit(
                    hidden_states=latents,
                    timestep=torch.full((1,), sigma, dtype=torch.bfloat16, device=device),
                    encoder_hidden_states=text,
                    encoder_hidden_states_mask=None,
                    img_shapes=[[(1, side, side)]],
                    attention_kwargs=options,
                    return_dict=False,
                )[0]
                latents = latents + (following - sigma) * velocity
                after(step)
        return latents.float().cpu()


class Lightning(Model[LoRAGraph]):
    graph: LoRAGraph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(LoRAGraph, factory=LoRAGraph)

    @uses_components(LORA_COMPONENT)
    def denoise(self, base: QwenDiT, inputs: Inputs, after: Any) -> torch.Tensor:
        """`base.denoise` with this LoRA handed to each of its forwards, as H3's `_sample` hands
        its overlay through `attention_kwargs` while its own scope holds the factors."""
        return base.denoise(inputs, self.graph.components[LORA_COMPONENT], after)


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 4242
    #: square image side in pixels (16 px a token)
    size: int = 512
    #: DiT forwards per call
    steps: int = 2
    #: GiB the ballast leaves the DiT's pages: less than the DiT, so it pages within each call
    room_gib: float = 6.0
    #: warm calls after the references, alternating LoRA and base
    rounds: int = 3


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(
    defaults={
        "model": "qwen/qwen-image-edit-2511@0.1.0",
        "turbo_lora": "lightx2v/qwen-image-edit-2511-lightning-lora@0.1.0",
    },
    components={"model": ("transformer",)},
)
def gpu(
    payload: Probe, model: QwenDiT, turbo_lora: Lightning, ctx: Context, tel: Telemetry
) -> Report:
    from .exercise import exercise

    result = exercise(payload, model, turbo_lora, ctx, tel)
    tel.log("lora overlay refill result", result=json.dumps(result, sort_keys=True))
    if not result["passed"]:
        keys = ("failures", "lora_effect", "bake_error", "runtime", "runtime_commit")
        brief = {k: result[k] for k in keys}
        raise RuntimeError(f"LoRA overlay refill failed: {json.dumps(brief, sort_keys=True)}")
    return Report(True, json.dumps(result, sort_keys=True))
