"""HunyuanImage 3.0's image stage, the package's port against Tencent's own code, on CPU with one small random-weight model
(no checkpoint): the same weights, prompt, references, size, steps and initial noise; the conditioning pictures' VAE
posteriors at their means on both sides. Each side runs in its own environment; `compare` reads both dumps.

  python hunyuan_forward.py ref  <Tencent's hunyuan_image_3 dir> <files dir> <instruct|distil> <out dir>   (Transformers 4.57.1)
  python hunyuan_forward.py port <hunyuan-image-3 dir> <files dir> <instruct|distil> <out dir>              (the package's env)
  python hunyuan_forward.py compare <out dir> <instruct|distil>

The ref run writes the weights the port loads. Cases: text to image, a one-reference edit and (Instruct) a two-reference
fusion, `bot_task="image"` at a fixed size; Instruct runs both CFG halves, Distil its guidance and meanflow tokens."""

import contextlib
import importlib.util
import json
import sys
import types
from pathlib import Path

import numpy as np
import torch

TINY = {
    "hidden_size": 64, "num_attention_heads": 4, "attention_head_dim": 16, "head_dim": 16,
    "num_key_value_heads": 2, "num_hidden_layers": 2, "intermediate_size": 48,
    "moe_intermediate_size": [32, 32], "moe_topk": [2, 2], "num_experts": 4, "num_shared_expert": [1, 1],
    "image_base_size": 256, "patch_embed_hidden_dim": 32,
}
CASES = [  # name, reference picture sizes, image_size, steps, seed
    ("t2i", [], "352x176", 3, 8),
    ("edit", [(300, 200)], "304x208", 3, 9),
    ("fusion", [(256, 256), (200, 320)], "256x256", 2, 10),
]
PROMPT = "A red fox in fresh snow, case {}"


def config_dict(files: Path, kind: str) -> dict:
    full = json.loads((files / "config.json").read_text())
    tiny = {**full, **TINY, "cfg_distilled": kind == "distil", "use_meanflow": kind == "distil"}
    tiny["vae"] = {**full["vae"], "block_out_channels": [32] * 5, "latent_channels": 16, "layers_per_block": 1}
    tiny["vit"] = {**full["vit"], "hidden_size": 32, "intermediate_size": 64, "num_attention_heads": 2, "num_hidden_layers": 1}
    tiny["vit_aligner"] = {**full["vit_aligner"], "input_dim": 32, "n_embed": 64}
    return tiny


def picture(seed: int, size: tuple[int, int]):
    from PIL import Image
    rng = np.random.default_rng(seed)
    return Image.fromarray(rng.integers(0, 255, (size[1], size[0], 3), dtype=np.uint8))


def noise_for(shape, seed: int) -> torch.Tensor:
    return torch.randn(tuple(shape), generator=torch.Generator().manual_seed(1000 + seed), dtype=torch.float32)


class CpuAutocast(torch.autocast):
    """Both trees open `torch.autocast("cuda", ...)` regions; on this CPU the same regions autocast on the CPU (bfloat16),
    so the bfloat16 modules take float32 latents as on a GPU. The VAE's float16 regions run in bfloat16 (CPU group norm
    takes no float16 beside bfloat16 weights)."""

    def __init__(self, device_type, dtype=None, *args, **kwargs):
        if device_type == "cuda":
            device_type, dtype = "cpu", torch.bfloat16 if dtype in (torch.float16, None) else dtype
        super().__init__(device_type, dtype, *args, **kwargs)


def load_tree(name: str, path: Path) -> None:
    package = types.ModuleType(name)
    package.__path__ = [str(path)]
    sys.modules[name] = package


def ref(tree: Path, files: Path, kind: str, out: Path) -> None:
    torch.autocast = CpuAutocast
    torch.cuda.set_device = lambda *a, **k: None
    torch.cuda.nvtx.range = lambda *a, **k: contextlib.nullcontext()
    load_tree("tencent", tree)
    modeling = importlib.import_module("tencent.modeling_hunyuan_image_3")
    configuration = importlib.import_module("tencent.configuration_hunyuan_image_3")
    pipeline = importlib.import_module("tencent.hunyuan_image_3_pipeline")
    vae_module = importlib.import_module("tencent.autoencoder_kl_3d")
    from transformers import GenerationConfig
    torch.manual_seed(0)
    config = configuration.HunyuanImage3Config(**config_dict(files, kind))
    config._attn_implementation = "sdpa"
    empty = torch.empty
    torch.empty = lambda *a, **k: empty(*a, **{**k, "device": "cpu"} if k.get("device") == "cuda" else k)
    try:
        model = modeling.HunyuanImage3ForCausalMM(config).eval()
    finally:
        torch.empty = empty
    model.load_tokenizer(str(files))
    model.generation_config = GenerationConfig(**json.loads((files / "generation_config.json").read_text()))
    model.to(torch.bfloat16)
    for layer in model.model.layers:  # Tencent's float32 routers of bfloat16 values
        layer.mlp.gate.wg.float()
    model.vae.float()
    type(model).dtype = property(lambda self: self.model.wte.weight.dtype)
    out.mkdir(parents=True, exist_ok=True)
    torch.save(model.state_dict(), out / f"weights-{kind}.pt")
    vae_module.DiagonalGaussianDistribution.sample = lambda self, generator=None: self.mean
    model.image_processor.postprocess_outputs = lambda outputs, **_: outputs
    seen: dict[str, torch.Tensor] = {}
    real = pipeline.randn_tensor

    def fixed(shape, generator=None, device=None, dtype=None, layout=None):
        return noise_for(shape, seen["seed"]).to(device=device or "cpu", dtype=dtype or torch.float32)

    pipeline.randn_tensor = fixed
    decode = model.vae.decode

    def keep(latents, *a, **k):
        seen["latents"] = latents.detach().clone()
        return decode(latents, *a, **k)

    model.vae.decode = keep
    dump = {}
    for name, sizes, image_size, steps, seed in CASES:
        if kind == "distil" and name == "fusion":
            continue
        seen["seed"] = seed
        model.generation_config.diff_infer_steps = steps
        _, (image,) = model.generate_image(
            prompt=PROMPT.format(name), image=[picture(seed + i, s) for i, s in enumerate(sizes)] or None, seed=seed,
            image_size=image_size, use_system_prompt="en_unified", bot_task="image", infer_align_image_size=True,
        )
        dump[name] = {"latents": seen["latents"].squeeze(2).float(), "image": torch.from_numpy(np.asarray(image).copy())}
        print(f"ref {kind} {name}: image {image.size}", flush=True)
    pipeline.randn_tensor = real
    torch.save(dump, out / f"ref-{kind}.pt")


def port(package: Path, files: Path, kind: str, out: Path) -> None:
    torch.autocast = CpuAutocast
    load_tree("hunyuan_image_3", package / "hunyuan_image_3")  # its modules, not its app's `__init__`
    from cozy_runtime import rng
    from hunyuan_image_3.model import GENERATION, _system, _tokenizer
    from hunyuan_image_3.tencent import autoencoder_kl_3d as vae_module
    from hunyuan_image_3.tencent.configuration_hunyuan_image_3 import HunyuanImage3Config
    from hunyuan_image_3.tencent.modeling_hunyuan_image_3 import HunyuanImage3ForCausalMM
    model = HunyuanImage3ForCausalMM(HunyuanImage3Config(**config_dict(files, kind))).eval()
    model.to(torch.bfloat16)
    model.vae.float()
    model.load_state_dict(torch.load(out / f"weights-{kind}.pt", weights_only=True), strict=True)
    model._tokenizer = _tokenizer(lambda name: (files / name).read_bytes())
    model.generation_config = types.SimpleNamespace(**GENERATION)
    vae_module.DiagonalGaussianDistribution.sample = lambda self, generator=None: self.mean
    seen = {}
    real = rng.noise

    def fixed(shape, seed, *, dtype=torch.float32, device="cpu"):
        return noise_for(shape, seen["seed"]).to(device=device, dtype=dtype)

    dump = {}
    for name, sizes, image_size, steps, seed in CASES:
        if kind == "distil" and name == "fusion":
            continue
        seen["seed"] = seed
        cond = None
        images = [picture(seed + i, s) for i, s in enumerate(sizes)]
        if images:
            cond = [model.image_processor.build_cond_images(image_list=images, infer_align_image_size=True)]
        # The package's denoise draws the image's noise through rng.noise (`modeling_hunyuan_image_3.denoise`).
        import hunyuan_image_3.tencent.modeling_hunyuan_image_3 as m
        m.rng.noise = fixed
        try:
            with torch.inference_mode():
                latents = model.denoise(PROMPT.format(name), cond, _system("image"), None, image_size=image_size,
                                        steps=steps, guidance_scale=GENERATION["diff_guidance_scale"], seed=seed)
                image = model.decode_latents(latents)
        finally:
            m.rng.noise = real
        dump[name] = {"latents": latents.float(), "image": torch.from_numpy(np.asarray(image).copy())}
        print(f"port {kind} {name}: image {image.size}", flush=True)
    torch.save(dump, out / f"port-{kind}.pt")


def compare(out: Path, kind: str) -> int:
    a, b = torch.load(out / f"ref-{kind}.pt"), torch.load(out / f"port-{kind}.pt")
    bad = 0
    for name in a:
        la, lb = a[name]["latents"], b[name]["latents"]
        ia, ib = a[name]["image"].int(), b[name]["image"].int()
        if la.shape != lb.shape or ia.shape != ib.shape:
            print(f"{kind} {name}: shapes {tuple(la.shape)} {tuple(ia.shape)} vs {tuple(lb.shape)} {tuple(ib.shape)}")
            bad += 1
            continue
        dl = float((la - lb).abs().max())
        di = int((ia - ib).abs().max())
        rel = dl / max(float(la.abs().max()), 1e-9)
        print(f"{kind} {name}: latents max |diff| {dl:.3g} ({rel:.2%} of max), image max |diff| {di} of 255, mean {float((ia - ib).abs().float().mean()):.3f}")
        bad += rel > 0.02
    return 1 if bad else 0


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "ref":
        ref(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4], Path(sys.argv[5]))
    elif mode == "port":
        port(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4], Path(sys.argv[5]))
    else:
        sys.exit(compare(Path(sys.argv[2]), sys.argv[3]))
