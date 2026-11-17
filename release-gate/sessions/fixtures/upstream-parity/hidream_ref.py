"""HiDream-O1-Image's own code (HiDream-ai/HiDream-O1-Image: models/pipeline.py `generate_image`), set up as its
inference.py sets it up (lines 66-119), one checkpoint loaded once for several cells. Attention takes the model's
SDPA route (`use_flash_attn` False, the README's route without flash-attn): the session's exact-attention reference.

  python hidream_ref.py <repo> <checkpoint dir> <full|dev> <cells.json> <out dir>

cells.json: [{"name", "prompt", "ref": path or null, "keep_original_aspect": bool, "shift": float (full only),
"seed": int, "height": int, "width": int}]. Writes <out>/<name>.png and <out>/<name>.json (size, seconds)."""

import json
import os
import sys
import time

repo, checkpoint, model_type, cells_path, out = sys.argv[1:6]
sys.path.insert(0, repo)

import torch  # noqa: E402
from transformers import AutoProcessor  # noqa: E402

from inference import add_special_tokens, get_tokenizer  # noqa: E402
from models.pipeline import DEFAULT_TIMESTEPS, generate_image  # noqa: E402
from models.qwen3_vl_transformers import Qwen3VLForConditionalGeneration  # noqa: E402

os.makedirs(out, exist_ok=True)
processor = AutoProcessor.from_pretrained(checkpoint)
t0 = time.time()
model = Qwen3VLForConditionalGeneration.from_pretrained(checkpoint, torch_dtype=torch.bfloat16, device_map="cuda").eval()
print(json.dumps({"loaded_s": round(time.time() - t0, 1), "attn": model.config._attn_implementation}), flush=True)
add_special_tokens(get_tokenizer(processor))

forward = model.forward


def sdpa_forward(*args, **kwargs):
    kwargs["use_flash_attn"] = False
    return forward(*args, **kwargs)


model.forward = sdpa_forward

for cell in json.load(open(cells_path)):
    refs = [cell["ref"]] if cell.get("ref") else []
    extra = {}
    if model_type == "full":  # inference.py lines 79-84
        steps, guidance, shift, timesteps, scheduler = 50, 5.0, cell.get("shift", 3.0), None, "default"
    elif len(refs) == 1:  # lines 86-92: Dev's one-picture edit, flow_match
        steps, guidance, shift, timesteps, scheduler = 28, 0.0, 1.0, DEFAULT_TIMESTEPS, "flow_match"
    else:  # lines 93-101: Dev's flash sampler
        steps, guidance, shift, timesteps, scheduler = 28, 0.0, 1.0, DEFAULT_TIMESTEPS, "flash"
        extra = {"noise_scale_start": 7.5, "noise_scale_end": 7.5, "noise_clip_std": 2.5}
    t0 = time.time()
    image = generate_image(
        model=model, processor=processor, prompt=cell["prompt"], ref_image_paths=refs, height=cell.get("height", 2048),
        width=cell.get("width", 2048), num_inference_steps=steps, guidance_scale=guidance, shift=shift,
        timesteps_list=timesteps, scheduler_name=scheduler, seed=cell["seed"],
        keep_original_aspect=cell.get("keep_original_aspect", False), layout_bboxes=None, **extra,
    )
    torch.cuda.synchronize()
    seconds = round(time.time() - t0, 1)
    image.save(os.path.join(out, cell["name"] + ".png"))
    facts = {"cell": cell["name"], "size": list(image.size), "seconds": seconds, "steps": steps, "shift": shift,
             "guidance": guidance, "scheduler": scheduler, "peak_gb": round(torch.cuda.max_memory_allocated() / 1e9, 1)}
    json.dump(facts, open(os.path.join(out, cell["name"] + ".json"), "w"))
    print(json.dumps(facts), flush=True)
