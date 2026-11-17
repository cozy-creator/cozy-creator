"""What Ideogram 4's two latent denormalizations do to a real picture, on CPU with the real VAE (no DiT): the picture's FLUX.2
latents x, packed into the DiT's 128 channels, normalized as Ideogram's pipeline assumes its DiT makes them, z = (x - SHIFT) / SCALE
(Ideogram's latent_norm.py). Ideogram's decode undoes that exactly (x); Diffusers' Ideogram4Pipeline and ComfyUI undo it with the
VAE's BatchNorm statistics instead, x' = z * bn_std + bn_mean. Both decoded; PSNR, mean colour per channel, and a crop sheet.

  python ideogram_norm.py <vae dir (diffusers AutoencoderKLFlux2)> <ideogram4 src dir> <picture> <out dir>"""

import json
import math
import sys
from pathlib import Path

import numpy as np
import torch
from diffusers.models.autoencoders.autoencoder_kl_flux2 import AutoencoderKLFlux2
from PIL import Image

vae_dir, src, picture, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
out.mkdir(parents=True, exist_ok=True)
ns: dict = {}
exec((src / "ideogram4" / "latent_norm.py").read_text().split("def get_latent_norm")[0].replace("import torch", ""), ns)
shift, scale = torch.tensor(ns["LATENT_SHIFT"]), torch.tensor(ns["LATENT_SCALE"])
vae = AutoencoderKLFlux2.from_pretrained(str(vae_dir), torch_dtype=torch.float32).eval()
bn_mean = vae.bn.running_mean.float()
bn_std = torch.sqrt(vae.bn.running_var.float() + vae.config.batch_norm_eps)

image = Image.open(picture).convert("RGB")
w, h = image.size
s = math.sqrt(1024 * 1024 / (w * h))
w, h = int(w * s) // 16 * 16, int(h * s) // 16 * 16
image = image.resize((w, h), Image.LANCZOS)
x = torch.from_numpy(np.asarray(image, dtype=np.float32) / 127.5 - 1).permute(2, 0, 1)[None]


def pack(lat: torch.Tensor) -> torch.Tensor:  # [1, 32, 2H, 2W] -> [1, H*W, 128] in (2, 2, 32) order, as the DiT's tokens
    c, gh, gw = lat.shape[1], lat.shape[2] // 2, lat.shape[3] // 2
    return lat.view(1, c, gh, 2, gw, 2).permute(0, 2, 4, 3, 5, 1).reshape(1, gh * gw, 4 * c)


def unpack(z: torch.Tensor, gh: int, gw: int) -> torch.Tensor:  # the pipelines' own unpatch
    c = z.shape[-1] // 4
    return z.view(1, gh, gw, 2, 2, c).permute(0, 5, 1, 3, 2, 4).reshape(1, c, gh * 2, gw * 2)


def to_image(pixels: torch.Tensor) -> np.ndarray:
    return ((pixels[0].clamp(-1, 1) + 1) * 127.5).round().permute(1, 2, 0).numpy().astype(np.uint8)


with torch.no_grad():
    raw = vae.encode(x).latent_dist.mode()
    gh, gw = raw.shape[2] // 2, raw.shape[3] // 2
    packed = pack(raw)
    assert torch.allclose(unpack(packed, gh, gw), raw)
    z = (packed - shift) / scale  # what Ideogram's DiT is trained to make, by Ideogram's latent_norm
    ideogram = to_image(vae.decode(unpack(z * scale + shift, gh, gw)).sample)
    diffusers = to_image(vae.decode(unpack(z * bn_std + bn_mean, gh, gw)).sample)
source = np.asarray(image)
facts = {"size": [w, h]}
for name, a, b in (("ideogram_decode_vs_source", source, ideogram), ("diffusers_decode_vs_ideogram_decode", ideogram, diffusers)):
    mse = float(((a.astype(np.float64) - b) ** 2).mean())
    facts[name + "_psnr_db"] = round(10 * math.log10(255**2 / mse), 2)
facts["mean_rgb"] = {k: [round(float(v), 1) for v in img.reshape(-1, 3).mean(0)] for k, img in
                     (("source", source), ("ideogram", ideogram), ("diffusers", diffusers))}
facts["std_rgb"] = {k: [round(float(v), 1) for v in img.reshape(-1, 3).std(0)] for k, img in
                    (("source", source), ("ideogram", ideogram), ("diffusers", diffusers))}
stem = picture.stem
json.dump(facts, open(out / f"{stem}-norm.json", "w"), indent=1)
print(json.dumps(facts))
W = 384
y, xo = (h - W) // 2, (w - W) // 2
tiles = [img[y:y + W, xo:xo + W] for img in (source, ideogram, diffusers)]
tiles.append(np.clip(np.abs(ideogram.astype(np.int16) - diffusers) * 4, 0, 255).astype(np.uint8))
sheet = Image.fromarray(np.concatenate(tiles, axis=1))
sheet.resize((sheet.width * 2, sheet.height * 2), Image.NEAREST).save(out / f"{stem}-norm-crops.jpg", quality=92)
Image.fromarray(np.concatenate([source, ideogram, diffusers], axis=1)).save(out / f"{stem}-norm-full.jpg", quality=90)
