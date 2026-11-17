"""Ideogram 4's own code (ideogram-oss/ideogram4: src/ideogram4/pipeline_ideogram4.py), run as run_inference.py runs
it (lines 179-196), on the fp8 weights (Ideogram's weight-only fp8: each linear dequantized to bf16 per call), with
three stand-ins the session's comparison needs, none of them in the maths:

- the text encoder is stock Qwen3-VL-8B-Instruct in bf16 (Ideogram's own fp8 text encoder is its per-row fp8), so it
  matches the cozy side's bf16 encoder; the tokenizer stays Ideogram's;
- the initial noise is drawn as the cozy side draws it, `torch.randn` on a CPU generator seeded with the seed (Ideogram
  draws it on the device's generator, which no other code reproduces);
- the final latents are also decoded with the VAE's BatchNorm statistics in place of Ideogram's own latent
  normalization (`latent_norm.py`), which is what Diffusers' Ideogram4Pipeline (and the cozy package) decode with.

  python ideogram_ref.py <repo> <weights repo id> <qwen dir> <cells.json> <out dir>

cells.json: [{"name", "caption", "seed", "height", "width", "preset"}]. Writes <out>/<name>.png (Ideogram's decode),
<out>/<name>-bn.png (the BatchNorm decode), <out>/<name>.pt (the final latents) and <out>/<name>.json."""

import json
import os
import sys
import time

repo, weights_repo, qwen, cells_path, out = sys.argv[1:6]
sys.path.insert(0, os.path.join(repo, "src"))

import torch  # noqa: E402
from PIL import Image  # noqa: E402
from transformers import AutoTokenizer, Qwen3VLForConditionalGeneration  # noqa: E402

import ideogram4.pipeline_ideogram4 as pipeline  # noqa: E402
from ideogram4 import PRESETS  # noqa: E402

os.makedirs(out, exist_ok=True)


def stock_qwen(repo_id, device, dtype, *, tokenizer_subfolder=None, text_encoder_subfolder=None):
    tokenizer = AutoTokenizer.from_pretrained(repo_id, subfolder=tokenizer_subfolder)
    # Qwen's checkpoint is the whole Qwen3-VL (`model.*`, `lm_head`): its Qwen3VLModel is what Ideogram's encoder reads.
    whole, info = Qwen3VLForConditionalGeneration.from_pretrained(qwen, dtype=dtype, output_loading_info=True)
    assert not info["missing_keys"] and not info["unexpected_keys"], info
    return tokenizer, whole.model.to(device).eval()


pipeline._load_qwen3_vl = stock_qwen
t0 = time.time()
pipe = pipeline.Ideogram4Pipeline.from_pretrained(
    config=pipeline.Ideogram4PipelineConfig(weights_repo=weights_repo), device="cuda", dtype=torch.bfloat16,
)
print(json.dumps({"loaded_s": round(time.time() - t0, 1), "text_encoder": type(pipe.text_encoder).__name__}), flush=True)

decoded: dict[str, torch.Tensor] = {}
decode = pipe._decode


def keep_latents(z, *, grid_h, grid_w):
    decoded["z"] = z.detach().clone()
    return decode(z, grid_h=grid_h, grid_w=grid_w)


pipe._decode = keep_latents

for cell in json.load(open(cells_path)):
    preset = PRESETS[cell.get("preset", "V4_QUALITY_48")]
    height, width = cell.get("height", 1024), cell.get("width", 1024)
    tokens = (height // 16) * (width // 16)
    noise = torch.randn((1, tokens, 128), generator=torch.Generator().manual_seed(cell["seed"]), dtype=torch.float32)
    randn = torch.randn

    def cpu_noise(*args, **kwargs):
        if tuple(args) == (1, tokens, 128):
            return noise.to(kwargs.get("device", "cpu"), kwargs.get("dtype", torch.float32))
        return randn(*args, **kwargs)

    torch.randn = cpu_noise
    t0 = time.time()
    try:
        images = pipe(cell["caption"], height=height, width=width, num_steps=preset.num_steps,
                      guidance_schedule=preset.guidance_schedule, mu=preset.mu, std=preset.std, seed=cell["seed"],
                      raise_on_caption_issues=False)
    finally:
        torch.randn = randn
    torch.cuda.synchronize()
    seconds = round(time.time() - t0, 1)
    images[0].save(os.path.join(out, cell["name"] + ".png"))
    z = decoded["z"]
    torch.save(z.cpu(), os.path.join(out, cell["name"] + ".pt"))
    # The same latents through Diffusers' denormalization: the VAE's BatchNorm statistics, then Ideogram's own unpatch,
    # decoder and rounding (its `_decode` with the statistics swapped).
    bn = pipe.autoencoder.bn
    shift, scale = pipe.latent_shift, pipe.latent_scale
    pipe.latent_shift = bn.running_mean.float().to(z.device)
    pipe.latent_scale = torch.sqrt(bn.running_var.float() + bn.eps).to(z.device)
    try:
        decode(z, grid_h=height // 16, grid_w=width // 16)[0].save(os.path.join(out, cell["name"] + "-bn.png"))
    finally:
        pipe.latent_shift, pipe.latent_scale = shift, scale
    facts = {"cell": cell["name"], "size": [width, height], "seconds": seconds, "steps": preset.num_steps,
             "bn_eps": bn.eps, "peak_gb": round(torch.cuda.max_memory_allocated() / 1e9, 1),
             "max_abs_shift_minus_bn_mean": float((shift - bn.running_mean.float().to(shift.device)).abs().max())}
    json.dump(facts, open(os.path.join(out, cell["name"] + ".json"), "w"))
    print(json.dumps(facts), flush=True)
    Image.open(os.path.join(out, cell["name"] + ".png")).verify()
