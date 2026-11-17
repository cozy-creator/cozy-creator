"""MiniMax-H3-Image and P-H3-Image as upstream runs them (se-156): Diffusers 0.43's modular pipelines, bf16, SDPA, no
prompt upsampling, the DiT and text encoder group-offloaded onto a card smaller than the 66 GB DiT. MiniMax-H3-Image's
text encoder is MiniMax-H3's (its 14 shards are byte-identical, se-146), so it is read from there.

    python upstream_image.py cases.json   # {"model": "minimax-h3-image"|"p-h3-image", "out": dir,
                                          #  "cases": [{name, prompt, seed, height?, width?, images?: [path]}]}

Each case writes <out>/<name>/: image.png, prompt.txt and done.json (or error.txt).
"""

import gc
import json
import os
import sys
import time
import traceback

import numpy as np
import torch
from diffusers import ModularPipeline
from diffusers.hooks import apply_group_offloading
from PIL import Image

REPO = {"minimax-h3-image": "MiniMaxAI/MiniMax-H3-Image", "p-h3-image": "PrunaAI/P-H3-Image"}
NAMES = {
    "minimax-h3-image": ["tokenizer", "processor", "vae", "transformer", "scheduler"],
    "p-h3-image": ["text_encoder", "text_connector", "tokenizer", "processor", "vae", "transformer", "scheduler",
                   "audio_scheduler"],
}


def say(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


def main():
    spec = json.load(open(sys.argv[1]))
    model, out = spec["model"], spec["out"]
    dev = torch.device("cuda")
    available = int(next(line.split()[1] for line in open("/proc/meminfo") if line.startswith("MemAvailable"))) // (1 << 20)
    say(f"{model}; host RAM available {available} GiB; GPU {torch.cuda.get_device_name(0)}")
    if model == "p-h3-image":
        from diffusers.modular_pipelines.p_h3_image.modular_blocks_p_h3_image import PH3ImageBlocks

        blocks = PH3ImageBlocks()
        blocks.sub_blocks.pop("prompt_upsampler")  # the prompt as given (prompt_upsampler.py's own way)
        pipe = blocks.init_pipeline(REPO[model])
    else:
        pipe = ModularPipeline.from_pretrained(REPO[model])
    say(f"blocks {list(pipe.blocks.sub_blocks)}")
    t0 = time.time()
    pipe.load_components(names=NAMES[model], dtype=torch.bfloat16)
    if model == "minimax-h3-image":
        from transformers import Qwen3VLForConditionalGeneration

        pipe.update_components(text_encoder=Qwen3VLForConditionalGeneration.from_pretrained(
            "MiniMaxAI/MiniMax-H3", subfolder="text_encoder", dtype=torch.bfloat16))
    say(f"loaded in {time.time() - t0:.0f} s; config {dict((k, v) for k, v in pipe.config.items() if not isinstance(v, (list, tuple)) or k.endswith('sigmas'))}")
    say(f"scheduler {dict(pipe.scheduler.config)}")
    disk = None if available > 150 else os.path.join(out, "offload-te")
    apply_group_offloading(pipe.text_encoder, onload_device=dev, offload_type="leaf_level", use_stream=True,
                           low_cpu_mem_usage=True, offload_to_disk_path=disk)
    pipe.transformer.enable_group_offload(onload_device=dev, offload_type="block_level", num_blocks_per_group=1,
                                          use_stream=True, low_cpu_mem_usage=True)
    for name in ("vae", "text_connector"):
        if getattr(pipe, name, None) is not None:
            getattr(pipe, name).to(dev)
    for case in spec["cases"]:
        where = os.path.join(out, case["name"])
        os.makedirs(where, exist_ok=True)
        try:
            run(pipe, case, where)
        except Exception:
            open(os.path.join(where, "error.txt"), "w").write(traceback.format_exc())
            say(f"{case['name']}: FAILED\n{traceback.format_exc()}")
        gc.collect()
        torch.cuda.empty_cache()


def run(pipe, case, where):
    generator = torch.Generator().manual_seed(int(case["seed"]))  # `cozy_runtime.rng.generator(seed)`
    kwargs = dict(prompt=case["prompt"], generator=generator, output_type="np")
    if case.get("images"):
        kwargs["image"] = [Image.open(p).convert("RGB") for p in case["images"]]
    if case.get("height"):
        kwargs.update(height=int(case["height"]), width=int(case["width"]))
    open(os.path.join(where, "prompt.txt"), "w").write(case["prompt"])
    torch.cuda.reset_peak_memory_stats()
    t0 = time.time()
    with torch.no_grad():
        state = pipe(**kwargs)
    took = time.time() - t0
    image = np.asarray(state.get("images")[0])
    Image.fromarray((image * 255.0).round().clip(0, 255).astype(np.uint8)).save(os.path.join(where, "image.png"))
    facts = {"seconds": round(took, 1), "size": [int(image.shape[1]), int(image.shape[0])],
             "canvas": [state.get("width"), state.get("height")],
             "peak_gib": round(torch.cuda.max_memory_allocated() / 2**30, 1),
             "steps": [float(t) for t in state.get("timesteps")]}
    for key in ("reference_images", "vision_images"):
        got = state.get(key)
        if got:
            facts[key] = [list(i.size) for i in got]
    json.dump(facts, open(os.path.join(where, "done.json"), "w"))
    say(f"{case['name']}: {facts}")


if __name__ == "__main__":
    main()
