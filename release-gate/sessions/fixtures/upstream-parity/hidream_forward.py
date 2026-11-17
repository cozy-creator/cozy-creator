"""HiDream-O1-Image, the package's model against HiDream's own code (models/pipeline.py `generate_image`), on CPU with one small
random-weight Qwen3-VL (no checkpoint): the same weights, processor files, prompts, references, sizes, steps and seed (both
draw the initial noise from a CPU generator seeded with seed + 1). Each side in its own environment; `compare` reads both.

  python hidream_forward.py ref  <HiDream repo> <files dir> <out dir>    (Transformers 4.57.1)
  python hidream_forward.py port <hidream-o1 dir> <files dir> <out dir>  (the package's env)
  python hidream_forward.py compare <out dir>

Cases: the full model's text to image (UniPC, shift 3, CFG 5 to the end, as upstream), its one-reference edit (upstream's
keep_original_aspect), its two-reference composition (shift 1) and the Dev model's one-reference edit (flow-match Euler). The Dev
model's text to image is left out: its flash sampler's per-step noise is the package's own by design (README)."""

import json
import math
import sys
import types
from pathlib import Path

import numpy as np
import torch

STEPS = 8  # UniPC's: a short schedule exercises the same solver as 50


def tiny_config(files: Path) -> dict:
    c = json.loads((files / "config.json").read_text())
    t = c["text_config"]
    t.update(hidden_size=64, intermediate_size=128, num_hidden_layers=2, num_attention_heads=4, num_key_value_heads=2, head_dim=16)
    rope = t.get("rope_scaling") or t.get("rope_parameters") or {}
    rope["mrope_section"] = [2, 3, 3]
    v = c["vision_config"]
    v.update(depth=2, hidden_size=32, intermediate_size=64, num_heads=2, out_hidden_size=64, deepstack_visual_indexes=[0])
    return c


def picture(seed: int, size: tuple[int, int]):
    from PIL import Image
    rng = np.random.default_rng(seed)
    base = rng.integers(0, 255, (size[1] // 32 + 1, size[0] // 32 + 1, 3), dtype=np.uint8)
    return Image.fromarray(base).resize(size, Image.BILINEAR)  # smooth enough that resampling matters


CASES = [  # name, model, reference sizes, (width, height) of the output, seed, shift
    ("full-t2i", "full", [], (2048, 2048), 3, 3.0),
    ("full-edit", "full", [(1792, 2304)], None, 4, 3.0),
    ("full-two", "full", [(1600, 1200), (1024, 1536)], (2048, 2048), 5, 1.0),
    ("dev-edit", "dev", [(1792, 2304)], None, 6, 1.0),
]
PROMPT = "a red fox sitting in fresh snow, case {}"


def ref(repo: Path, files: Path, out: Path) -> None:
    sys.path.insert(0, str(repo))
    from transformers import AutoProcessor, Qwen3VLConfig
    from inference import add_special_tokens, get_tokenizer
    from models.pipeline import DEFAULT_TIMESTEPS, generate_image
    from models.qwen3_vl_transformers import Qwen3VLForConditionalGeneration
    processor = AutoProcessor.from_pretrained(str(files))
    add_special_tokens(get_tokenizer(processor))
    out.mkdir(parents=True, exist_ok=True)
    dump = {}
    for kind in ("full", "dev"):
        torch.manual_seed(0 if kind == "full" else 1)
        config = Qwen3VLConfig(**tiny_config(files))
        config._attn_implementation = "sdpa"
        model = Qwen3VLForConditionalGeneration(config).to(torch.bfloat16).eval()
        forward = model.forward
        model.forward = lambda *a, _f=forward, **k: _f(*a, **{**k, "use_flash_attn": False})
        torch.save(model.state_dict(), out / f"hidream-weights-{kind}.pt")
        for name, which, refs, size, seed, shift in CASES:
            if which != kind:
                continue
            paths = []
            for i, s in enumerate(refs):
                p = out / f"{name}-ref{i}.png"
                picture(seed * 10 + i, s).save(p)
                paths.append(str(p))
            w, h = size or (2048, 2048)
            if kind == "full":
                args = dict(num_inference_steps=STEPS, guidance_scale=5.0, shift=shift, timesteps_list=None, scheduler_name="default")
            else:
                args = dict(num_inference_steps=28, guidance_scale=0.0, shift=1.0, timesteps_list=DEFAULT_TIMESTEPS,
                            scheduler_name="flow_match")
            image = generate_image(model=model, processor=processor, prompt=PROMPT.format(name), ref_image_paths=paths,
                                   height=h, width=w, seed=seed, keep_original_aspect=size is None and len(paths) == 1, **args)
            dump[name] = torch.from_numpy(np.asarray(image).copy())
            print(f"ref {name}: {image.size}", flush=True)
    torch.save(dump, out / "hidream-ref.pt")


def port(package: Path, files: Path, out: Path) -> None:
    import inspect
    package_module = types.ModuleType("hidream_o1")
    package_module.__path__ = [str(package / "hidream_o1")]
    sys.modules["hidream_o1"] = package_module  # its modules, not its app's `__init__`
    from cozy_runtime import vlm
    from hidream_o1.model import HiDreamO1, Recipe, Root, _layers, area_resize, reference_size
    from cozy_runtime.attention import Attention
    from PIL import Image
    from transformers import Qwen3VLConfig
    generate = inspect.unwrap(HiDreamO1.generate)
    processor = vlm.processor(files)
    dump = {}
    for kind in ("full", "dev"):
        root = Root(Qwen3VLConfig(**tiny_config(files))).to(torch.bfloat16).eval()  # O1Graph's construction, bfloat16 throughout
        text = root.model.language_model.config
        for layer in _layers(root.model.language_model):
            layer.self_attn.prefix_site = Attention(text.num_attention_heads, text.head_dim, causal=True)
            layer.self_attn.site = Attention(text.num_attention_heads, text.head_dim)
        state = torch.load(out / f"hidream-weights-{kind}.pt", weights_only=True)
        missing, unexpected = root.load_state_dict(state, strict=False)
        assert not missing and unexpected == ["lm_head.weight"], (missing, unexpected)
        model = types.SimpleNamespace(graph=types.SimpleNamespace(components={"model": root}), processor=processor, device="cpu",
                                      body=root.model)
        for name, which, refs, size, seed, shift in CASES:
            if which != kind:
                continue
            images = [Image.open(out / f"{name}-ref{i}.png") for i in range(len(refs))]
            if size is None:  # the package's `layout` for one reference and no size: the reference's own pixels
                lone = area_resize(images[0], math.sqrt(images[0].width * images[0].height))
                (w, h), prepared = lone.size, [lone]
            else:
                w, h = size
                side = reference_size(max(w, h), len(images))
                prepared = [area_resize(im, side) for im in images]
            if kind == "full":
                recipe, steps, guidance, negative = Recipe("unipc", shift), STEPS, 5.0, " "
            else:
                recipe, steps, guidance, negative = Recipe("euler"), 28, 0.0, None
            with torch.inference_mode():
                image = generate(model, PROMPT.format(name), negative, prepared, width=w, height=h, recipe=recipe, steps=steps,
                                 guidance=guidance, seed=seed, progress=lambda i, n: None, interval=False)
            dump[name] = torch.from_numpy(np.asarray(image).copy())
            print(f"port {name}: {image.size}", flush=True)
    torch.save(dump, out / "hidream-port.pt")


def compare(out: Path) -> int:
    a, b = torch.load(out / "hidream-ref.pt"), torch.load(out / "hidream-port.pt")
    bad = 0
    for name in a:
        x, y = a[name].double(), b[name].double()
        if x.shape != y.shape:
            print(f"{name}: sizes {tuple(x.shape)} vs {tuple(y.shape)}")
            bad += 1
            continue
        mse = float(((x - y) ** 2).mean())
        psnr = math.inf if mse == 0 else 10 * math.log10(255**2 / mse)
        print(f"{name}: PSNR {psnr:.1f} dB, max |diff| {int((x - y).abs().max())}, mean {float((x - y).abs().mean()):.3f}")
        bad += psnr < 30
    return 1 if bad else 0


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "ref":
        ref(Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]))
    elif mode == "port":
        port(Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]))
    else:
        sys.exit(compare(Path(sys.argv[2])))
