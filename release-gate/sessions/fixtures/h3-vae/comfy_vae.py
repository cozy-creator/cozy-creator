"""ComfyUI's MiniMax H3 video VAE timed on this pod's GPU (sessions/h3-vae.sh, cr-306): its VAEEncode and VAEDecode
nodes' work on each clip, started as ComfyUI's main.py starts (its allocator, dynamic VRAM, `--fast`), one warm-up and
then `runs` timed passes, each synchronized. Prints one JSON line a pass.

argv: <ComfyUI dir> <VAE file> <runs> <fast: 0|1> <clip.mp4>...; $COMFY_ARGS: more of main.py's flags.
"""

import faulthandler
import json
import logging
import os
import sys
import time
from importlib import import_module

faulthandler.enable()
logging.basicConfig(level=logging.INFO, stream=sys.stderr)

root, vae_file, runs, fast = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4] == "1"
clips = sys.argv[5:]
sys.path.insert(0, root)
os.chdir(root)
sys.argv = ["main.py", *(["--fast", "fp16_accumulation"] if fast else []), *os.environ.get("COMFY_ARGS", "").split()]

# ComfyUI's modules come from its checkout (argv), so they are imported by name.
import_module("comfy.options").enable_args_parsing()
cli = import_module("comfy.cli_args")
args, dynamic = cli.args, cli.enables_dynamic_vram()
import_module("cuda_malloc")  # main.py's allocator, set before torch loads
control = import_module("comfy_aimdo.control")
if dynamic:
    headroom = None if args.reserve_vram is None else int(args.reserve_vram * 1024**3)
    control.init(simple_vram_headroom=headroom, nvml_pressure=not args.disable_nvml_pressure)

import av  # noqa: E402
import numpy as np  # noqa: E402
import torch  # noqa: E402

memory = import_module("comfy.memory_management")
mm = import_module("comfy.model_management")
patcher = import_module("comfy.model_patcher")
if dynamic:
    try:
        started = control.init_devices((d.index, int(args.vram_headroom * 1024**3)) for d in mm.get_all_torch_devices())
    except TypeError:
        started = control.init_devices(d.index for d in mm.get_all_torch_devices())
    if started:
        setattr(patcher, "CoreModelPatcher", patcher.ModelPatcherDynamic)  # noqa: B010
        setattr(memory, "aimdo_enabled", True)  # noqa: B010
sd_module, utils, nodes = import_module("comfy.sd"), import_module("comfy.utils"), import_module("nodes")


def frames(path: str) -> torch.Tensor:
    with av.open(path, "r") as container:
        rgb = [f.to_ndarray(format="rgb24") for f in container.decode(video=0)]
    return torch.from_numpy(np.stack(rgb)).float().div_(255.0)


sd, metadata = utils.load_torch_file(vae_file, return_metadata=True)
vae = sd_module.VAE(sd=sd, metadata=metadata)
facts = {
    "torch": torch.__version__,
    "fp16_accumulation": bool(getattr(torch.backends.cuda.matmul, "allow_fp16_accumulation", False)),
    "vae_dtype": str(vae.vae_dtype),
    "aimdo": bool(memory.aimdo_enabled),
    "argv": sys.argv[1:],
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
