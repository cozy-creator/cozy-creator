"""Qwen-Image-Edit-2511's DiT (its int8 lane, the Lightning LoRA's `lora.Hook` on every site) in a plain
step loop, armed before the loop as Qwen-Image's package arms. Three calls in one run: unarmed twice,
then armed; the armed latents equal the same call with graphs bypassed, byte for byte, and differ from
the unarmed ones. `inside` arms in the step instead: the latents equal those armed before the loop.
Until Runtime 0.43 the loop was a keyed `denoise_loop` whose kept program had to tell armed from
unarmed; Runtime 0.43 has no loop programs."""

from __future__ import annotations

import json
import time
from functools import partial
from typing import Any

import msgspec
import torch
from cozy_runtime import graphs, rng
from cozy_runtime.accel import lora
from cozy_runtime.author import App, Config, Context, Loader, Model, Telemetry, uses_components
from cozy_runtime.internal import kernel_ready, lora_bake
from diffusers import QwenImageTransformer2DModel
from torch import nn

app = App()

SITES = (
    "attn.to_q", "attn.to_k", "attn.to_v", "attn.to_out.0", "attn.add_q_proj", "attn.add_k_proj",
    "attn.add_v_proj", "attn.to_add_out", "img_mlp.net.0.proj", "img_mlp.net.2", "txt_mlp.net.0.proj",
    "txt_mlp.net.2",
)
WIDTH, HIDDEN, BLOCKS, RANK = 3072, 12288, 60, 64
LORA_COMPONENT = "model"
_DTYPES = {"f32": torch.float32, "bf16": torch.bfloat16, "f16": torch.float16, "i64": torch.int64}
#: latent side in tokens, text rows, and DiT forwards a call
SIDE, TEXT, STEPS = 32, 64, 3


def _armed(dit: nn.Module, path: str) -> nn.Module | None:
    arming = dit._ci_arming
    return None if arming is None else arming.get_submodule(path)


class DiTGraph:
    """The DiT in bf16 with a permanent `lora.Hook` per site; a call arms it before its loop."""

    def __init__(self, config: Config) -> None:
        dit = QwenImageTransformer2DModel.from_config(config.section("transformer").mapping())
        dit = dit.to(dtype=torch.bfloat16)
        dit._ci_arming = None
        for index in range(len(dit.transformer_blocks)):
            for site in SITES:
                path = f"transformer_blocks.{index}.{site}"
                dit.get_submodule(path).register_forward_hook(lora.Hook(partial(_armed, dit, path)))
        self.components: dict[str, nn.Module] = {"transformer": dit}


class Factors(nn.Module):
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
            return _DTYPES[config.tensor_dtype(f"{LORA_COMPONENT}.transformer_blocks.0.{SITES[0]}.{name}", default="f32")]

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
                sizes = ((WIDTH, HIDDEN) if site.endswith("net.0.proj") else (HIDDEN, WIDTH)
                         if site.endswith("net.2") else (WIDTH, WIDTH))
                parent.add_module(leaf, Factors(*sizes, weights, alpha))
            root.transformer_blocks.append(block)
        self.components: dict[str, nn.Module] = {LORA_COMPONENT: root}


class QwenDiT(Model[DiTGraph], encoded_leaves="accept"):
    graph: DiTGraph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(DiTGraph)

    @uses_components("transformer")
    def denoise(self, seed: int, overlay: nn.Module | None, inside: bool = False) -> torch.Tensor:
        """`STEPS` Euler steps from seeded noise and text rows in a plain loop, the DiT armed with
        `overlay` (or none) before it; the step reads its sigma at the step index,
        its rotary table and modulation rows made once for the layout."""
        dit = self.graph.components["transformer"]
        device = torch.device(str(self.device))
        latents = rng.noise((1, SIDE * SIDE, 64), seed, dtype=torch.bfloat16).to(device)
        text = rng.noise((1, TEXT, 3584), seed + 1, dtype=torch.bfloat16).to(device) * 0.1
        sigmas = torch.linspace(1.0, 1.0 / (STEPS + 1), STEPS + 1, device=device)
        rotary = dit.pos_embed([[(1, SIDE, SIDE)]], max_txt_seq_len=TEXT, device=device)
        modulate = (torch.zeros((1, SIDE * SIDE), dtype=torch.int, device=device)
                    if dit.zero_cond_t else None)

        def step(i: torch.Tensor, latents: torch.Tensor, text: torch.Tensor, sigmas: torch.Tensor) -> None:
            if inside:
                dit._ci_arming = overlay
            sigma = sigmas.index_select(0, i)
            hidden = dit.img_in(latents)
            timestep = sigma.to(hidden.dtype)
            if dit.zero_cond_t:
                timestep = torch.cat([timestep, timestep * 0], dim=0)
            encoder = dit.txt_in(dit.txt_norm(text))
            temb = dit.time_text_embed(timestep, hidden)
            for block in dit.transformer_blocks:
                encoder, hidden = block(hidden_states=hidden, encoder_hidden_states=encoder,
                                        encoder_hidden_states_mask=None, temb=temb, image_rotary_emb=rotary,
                                        joint_attention_kwargs=None, modulate_index=modulate)
            if dit.zero_cond_t:
                temb = temb.chunk(2, dim=0)[0]
            velocity = dit.proj_out(dit.norm_out(hidden, temb))
            latents.copy_(latents + (sigmas.index_select(0, i + 1) - sigma).to(latents.dtype) * velocity)

        dit._ci_arming = None if inside else overlay
        try:
            index = torch.arange(STEPS, device=device)
            for i in range(STEPS):
                step(index[i : i + 1], latents, text, sigmas)
        finally:
            dit._ci_arming = None
        return latents.float().cpu()


class Lightning(Model[LoRAGraph]):
    graph: LoRAGraph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(LoRAGraph)

    @uses_components(LORA_COMPONENT)
    def denoise(self, base: QwenDiT, seed: int, inside: bool = False) -> torch.Tensor:
        return base.denoise(seed, self.graph.components[LORA_COMPONENT], inside)


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 331
    #: replay: unarmed twice, then armed before the loop; inside: armed in the step
    variant: str = "replay"


class Report(msgspec.Struct):
    passed: bool
    detail: str


def _bake_kernels(device: torch.device, ctx: Context, tel: Telemetry) -> None:
    """A loop records only a baked LoRA (`_checked`): the bake kernels serve before this run."""
    kernel = f"{lora_bake.KERNELS.name} cuda:{device.index or 0}"
    began = time.monotonic()
    while (slot := kernel_ready._SLOTS.get(kernel)) is not None and slot.state == "opening":
        ctx.raise_if_cancelled()
        if time.monotonic() - began > 900:
            raise RuntimeError("the LoRA bake kernels did not build in 15 minutes")
        time.sleep(1)
    try:
        opener = partial(kernel_ready.TRITON[0], lora_bake.KERNELS, partial(lora_bake.probe, torch, device), device=device)
        kernel_ready.wait(kernel, opener)
    except Exception as error:
        tel.log("the LoRA bake kernels failed; baking eagerly", reason=str(error)[:300])


@app.entrypoint(
    defaults={
        "model": "qwen/qwen-image-edit-2511@0.1.0",
        "turbo_lora": "lightx2v/qwen-image-edit-2511-lightning-lora@0.1.0",
    },
    components={"model": ("transformer",)},
)
def gpu(payload: Probe, model: QwenDiT, turbo_lora: Lightning, ctx: Context, tel: Telemetry) -> Report:
    _bake_kernels(torch.device(str(model.device)), ctx, tel)
    if payload.variant == "inside":
        return _inside(payload, model, turbo_lora)
    model.denoise(payload.seed, None)
    model.denoise(payload.seed + 1, None)
    armed = turbo_lora.denoise(model, payload.seed + 2)
    eager = graphs.BYPASS.set(True)
    try:
        reference = turbo_lora.denoise(model, payload.seed + 2)
        base = model.denoise(payload.seed + 2, None)
    finally:
        graphs.BYPASS.reset(eager)
    checks: dict[str, Any] = {
        "armed_equals_eager": bool(torch.equal(armed, reference)),
        "lora_changes_it": not torch.equal(reference, base),
    }
    detail = json.dumps({"checks": checks}, sort_keys=True, default=str)
    if not all(checks.values()):
        raise RuntimeError(f"overlay replay {detail}")
    return Report(True, detail)


def _inside(payload: Probe, model: QwenDiT, turbo_lora: Lightning) -> Report:
    """An overlay the step arms gives the latents of one armed before the loop."""
    before = turbo_lora.denoise(model, payload.seed)
    inside = turbo_lora.denoise(model, payload.seed, inside=True)
    checks = {"inside_equals_before": bool(torch.equal(inside, before))}
    detail = json.dumps({"checks": checks}, sort_keys=True)
    if not all(checks.values()):
        raise RuntimeError(f"overlay inside a step {detail}")
    return Report(True, detail)
