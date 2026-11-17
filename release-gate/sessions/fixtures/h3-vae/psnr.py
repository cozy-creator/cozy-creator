"""PSNR of one `vae_round_trip` output against another's (sessions/h3-vae.sh, cr-306): `latents` files over the reference's
peak-to-peak range, `frames` files (decoded frames before clipping) over [0, 1], clipped as a video is. Prints one JSON line.

argv: latents|frames <reference file> <file>
"""

import json
import math
import sys

import numpy as np
from safetensors.numpy import load_file

kind, reference, candidate = sys.argv[1], load_file(sys.argv[2]), load_file(sys.argv[3])
a, b = reference[kind].astype(np.float64), candidate[kind].astype(np.float64)
if a.shape != b.shape:
    print(json.dumps({"kind": kind, "error": f"shapes {a.shape} and {b.shape}"}))
    sys.exit(1)
if kind == "frames":
    a, b, peak = np.clip(a, 0, 1), np.clip(b, 0, 1), 1.0
else:
    peak = float(a.max() - a.min())
mse = float(np.mean((a - b) ** 2))
psnr = math.inf if mse == 0 else 10 * math.log10(peak**2 / mse)
print(json.dumps({"kind": kind, "psnr_db": round(psnr, 2) if math.isfinite(psnr) else "inf",
                  "max_abs": float(np.max(np.abs(a - b))), "shape": list(a.shape)}))
