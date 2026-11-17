"""ComfyUI's MiniMax H3 video VAE timed on this pod's GPU (sessions/h3-vae.sh, cr-306): its VAEEncode and VAEDecode
nodes' work on each clip, started as ComfyUI's main.py starts (its allocator, dynamic VRAM, `--fast`), one warm-up and
then `runs` timed passes, each synchronized. Prints one JSON line a pass.

argv: <ComfyUI dir> <VAE file> <runs> <fast: 0|1> <clip.mp4>...
"""

import json
import os
import sys
import time

root, vae_file, runs, fast = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4] == "1"
clips = sys.argv[5:]
sys.path.insert(0, root)
os.chdir(root)
sys.argv = ["main.py", *(["--fast", "fp16_accumulation"] if fast else [])]

import comfy.options  # noqa: E402

comfy.options.enable_args_parsing()
from comfy.cli_args import args, enables_dynamic_vram  # noqa: E402

import cuda_malloc  # noqa: E402,F401  main.py's allocator, set before torch loads
import comfy_aimdo.control  # noqa: E402

if enables_dynamic_vram():
    headroom = None if args.reserve_vram is None else int(args.reserve_vram * 1024**3)
    comfy_aimdo.control.init(simple_vram_headroom=headroom, nvml_pressure=not args.disable_nvml_pressure)

import av  # noqa: E402
import numpy as np  # noqa: E402
import torch  # noqa: E402

import comfy.memory_management  # noqa: E402
import comfy.model_management as mm  # noqa: E402
import comfy.model_patcher  # noqa: E402

if enables_dynamic_vram():
    try:
        started = comfy_aimdo.control.init_devices(
            (d.index, int(args.vram_headroom * 1024**3)) for d in mm.get_all_torch_devices()
        )
    except TypeError:
        started = comfy_aimdo.control.init_devices(d.index for d in mm.get_all_torch_devices())
    if started:
        comfy.model_patcher.CoreModelPatcher = comfy.model_patcher.ModelPatcherDynamic
        comfy.memory_management.aimdo_enabled = True

import comfy.sd  # noqa: E402
import comfy.utils  # noqa: E402
import nodes  # noqa: E402


def frames(path: str) -> torch.Tensor:
    with av.open(path) as container:
        rgb = [f.to_ndarray(format="rgb24") for f in container.decode(video=0)]
    return torch.from_numpy(np.stack(rgb)).float().div_(255.0)


sd, metadata = comfy.utils.load_torch_file(vae_file, return_metadata=True)
vae = comfy.sd.VAE(sd=sd, metadata=metadata)
facts = {
    "torch": torch.__version__,
    "fp16_accumulation": bool(getattr(torch.backends.cuda.matmul, "allow_fp16_accumulation", False)),
    "vae_dtype": str(vae.vae_dtype),
    "aimdo": bool(comfy.memory_management.aimdo_enabled),
    "gpu": torch.cuda.get_device_name(),
}
print(json.dumps({"facts": facts}), flush=True)
for clip in clips:
    pixels = frames(clip)
    for i in range(runs + 1):
        torch.cuda.synchronize()
        t0 = time.perf_counter()
        (latent,) = nodes.VAEEncode().encode(vae, pixels)
        torch.cuda.synchronize()
        t1 = time.perf_counter()
        (images,) = nodes.VAEDecode().decode(vae, latent)
        torch.cuda.synchronize()
        t2 = time.perf_counter()
        print(json.dumps({
            "clip": os.path.basename(clip), "frames": int(pixels.shape[0]), "pass": i, "warmup": i == 0,
            "latent": list(latent["samples"].shape), "decoded": list(images.shape),
            "encode_s": round(t1 - t0, 3), "decode_s": round(t2 - t1, 3),
            "peak_gib": round(torch.cuda.max_memory_reserved() / 2**30, 2),
        }), flush=True)
