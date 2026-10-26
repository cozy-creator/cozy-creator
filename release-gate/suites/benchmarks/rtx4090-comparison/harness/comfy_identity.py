"""Untimed CPU normalization and schedule observation; calls libraries, copies no sampler math."""
import gc
import hashlib
import json
import subprocess
import sys
from pathlib import Path

B = Path("/var/tmp/bench4090-20261026")
COMFY = B / "ComfyUI"
checkpoint = COMFY / "models/checkpoints/sd_xl_base_1.0.safetensors"
assert subprocess.check_output(["git", "-C", str(COMFY), "rev-parse", "HEAD"], text=True).strip() == "fb3befd168731438f88bc96e0920a538e8649878"
sys.path.insert(0, str(COMFY))
sys.argv = ["identity", "--cpu"]
import torch
import comfy.options
comfy.options.enable_args_parsing()
import comfy.sd
import comfy.samplers

assert torch.__version__ == "2.14.1+cu130"
from huggingface_hub import HfApi
info = HfApi().model_info("stabilityai/stable-diffusion-xl-base-1.0", revision="462165984030d82259a11f4367a4eed129e94a7b", files_metadata=True)
expected = next(file.lfs.sha256 for file in info.siblings if file.rfilename == checkpoint.name)
assert hashlib.file_digest(checkpoint.open("rb"), "sha256").hexdigest() == expected
model, _, _, _ = comfy.sd.load_checkpoint_guess_config(str(checkpoint), output_vae=False, output_clip=False,
                                                   model_options={"dtype": torch.float16})
sampling = model.get_model_object("model_sampling")
sigmas = comfy.samplers.calculate_sigmas(sampling, "normal", 30)
assert sigmas.device.type == "cpu"
comfy_schedule = sigmas.tolist()
comfy_sampling_type = type(sampling).__name__
del model, sampling, sigmas
gc.collect()
from diffusers import StableDiffusionXLPipeline

# Normalize the exact same source bytes into the same published component layouts as TensorFS.
# No `.to(cuda)` or denoising runs. Tokenizer/config downloads are staged by setup before this.
pipe = StableDiffusionXLPipeline.from_single_file(str(checkpoint), config=str(B / "sdxl-config"),
                                                local_files_only=True, torch_dtype=torch.float16)
rows = {}
for name in ("unet", "vae", "text_encoder", "text_encoder_2"):
    rows[name] = {}
    for key, value in getattr(pipe, name).state_dict().items():
        value = value.detach().cpu().contiguous()
        dtype = {torch.float16: "F16", torch.float32: "F32", torch.bfloat16: "BF16",
                 torch.int64: "I64", torch.int32: "I32"}[value.dtype]
        rows[name][key] = {"dtype": dtype, "shape": list(value.shape),
                          "sha256": hashlib.sha256(value.view(torch.uint8).numpy().tobytes()).hexdigest()}
assert not torch.cuda.is_initialized(), "CPU preflight unexpectedly initialized CUDA"
receipt = {"comfy_commit": "fb3befd168731438f88bc96e0920a538e8649878", "torch": torch.__version__,
           "torch_git": torch.version.git_version, "components": rows,
           "sigmas": comfy_schedule, "comfy_sampling_type": comfy_sampling_type,
           "scheduler": "normal", "steps": 30,
           "checkpoint_sha256": hashlib.file_digest(checkpoint.open("rb"), "sha256").hexdigest(),
           "scope": "Actual Comfy model_sampling schedule; same-file Diffusers normalized tensor identity, CPU only. No claim of identical noise or kernel rounding."}
(B / "comfy-identity.json").write_text(json.dumps(receipt, sort_keys=True))
