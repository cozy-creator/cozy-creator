"""A candidate's outputs colour-matched to their input, as a new candidate: <runs>/<candidate>+<method>/<name>.png.

    colorfix.py <runs dir> <candidate> <method>[,<method>...]

The reference is the input (degraded/ or paul/) Lanczos-enlarged to the output. Methods, after SeedVR2's colour fix
(seedvr2/color.py, numz/ComfyUI-SeedVR2_VideoUpscaler): `adain`, each channel's mean and spread the reference's;
`wavelet`, the output's detail over the reference's low frequencies (five dilated 3x3 blurs, radius 1-16 px);
`wavelet64`, the same with seven (radius 1-64 px), only the broadest colour and light.
"""

import sys
from collections.abc import Callable
from pathlib import Path

import torch
from PIL import Image
from torch.nn import functional as F

import numpy as np

HERE = Path(__file__).resolve().parent
BLUR = torch.tensor([[1.0, 2.0, 1.0], [2.0, 4.0, 2.0], [1.0, 2.0, 1.0]]) / 16


def low(image: torch.Tensor, levels: int) -> torch.Tensor:
    kernel = BLUR.expand(image.shape[1], 1, 3, 3)
    for level in range(levels):
        radius = min(2**level, max(1, min(image.shape[-2:]) // 8))
        image = F.conv2d(F.pad(image, (radius,) * 4, mode="replicate"), kernel, groups=image.shape[1], dilation=radius)
    return image


def tensor(image: Image.Image) -> torch.Tensor:
    return torch.from_numpy(np.asarray(image, dtype=np.float32) / 255).permute(2, 0, 1)[None]


def adain(out: torch.Tensor, ref: torch.Tensor) -> torch.Tensor:
    m, v = out.mean((2, 3), keepdim=True), out.var((2, 3), keepdim=True)
    return (out - m) / (v + 1e-5).sqrt() * (ref.var((2, 3), keepdim=True) + 1e-5).sqrt() + ref.mean((2, 3), keepdim=True)


METHODS: dict[str, Callable[[torch.Tensor, torch.Tensor], torch.Tensor]] = {
    "adain": adain, "wavelet": lambda o, r: o - low(o, 5) + low(r, 5), "wavelet64": lambda o, r: o - low(o, 7) + low(r, 7),
}
runs, candidate = Path(sys.argv[1]), sys.argv[2]
for method in sys.argv[3].split(","):
    out_dir = runs / f"{candidate}+{method}"
    out_dir.mkdir(exist_ok=True)
    for path in sorted((runs / candidate).glob("*.png")):
        source = next((p for p in (HERE / "degraded" / f"{path.stem}.jpg", HERE / "paul" / f"{path.stem}.jpg") if p.exists()), None)
        if source is None:
            continue
        image = Image.open(path).convert("RGB")
        ref = Image.open(source).convert("RGB").resize(image.size, Image.Resampling.LANCZOS)
        fixed = METHODS[method](tensor(image), tensor(ref)).clamp(0, 1)[0].permute(1, 2, 0).mul(255).round().byte().numpy()
        Image.fromarray(fixed).save(out_dir / path.name)
    print(candidate, method, len(list(out_dir.glob("*.png"))), "pictures")
