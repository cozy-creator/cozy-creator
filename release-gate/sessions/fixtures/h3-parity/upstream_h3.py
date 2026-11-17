"""MiniMax-H3 as upstream runs it (se-156): Diffusers 0.43's modular pipeline, bf16, SDPA, Turbo through alibaba-pai's
`minimax_h3_pdd.py` (`MiniMax-H3-Acc-LoRAs`, LoRA 1.0, `num_inference_steps = nfe + 1`), the components group-offloaded
onto a card smaller than the 66 GB DiT. One process per trunk: its cases share the loaded components.

    python upstream_h3.py cases.json   # {"trunk": "fl2va"|"ref2va", "out": dir, "cases": [{name, prompt, seed,
                                       #   frames, height, width, image?, references?: [path], short_edge?}]}

Each case writes <out>/<name>/: latents.npy (the denoised, normalized (1, 24, T, h, w) video latents), frames.npy
((T, H, W, 3) uint8), audio.npy ((2, N) float32) with rate.txt, prompt.txt, and done.json (or error.txt).
"""

import gc
import json
import os
import random
import sys
import time
import traceback

import numpy as np
import torch
from diffusers import ModularPipeline
from diffusers.hooks import apply_group_offloading
from diffusers.modular_pipelines.minimax_h3 import MiniMaxH3ImageReference
from huggingface_hub import hf_hub_download
from PIL import Image

REPO = "MiniMaxAI/MiniMax-H3"
ACC = "alibaba-pai/MiniMax-H3-Acc-LoRAs"
LORA = {"fl2va": "MiniMax-H3-FL2VA-Acc-8Step.safetensors", "ref2va": "MiniMax-H3-Ref2VA-Acc-8Step.safetensors"}
DIT = {"fl2va": "transformer", "ref2va": "transformer_ref"}


def say(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


def main():
    spec = json.load(open(sys.argv[1]))
    trunk, out = spec["trunk"], spec["out"]
    dev = torch.device("cuda")
    available = int(next(line.split()[1] for line in open("/proc/meminfo") if line.startswith("MemAvailable"))) // (1 << 20)
    say(f"trunk {trunk}; host RAM available {available} GiB; GPU {torch.cuda.get_device_name(0)}")
    pipe = ModularPipeline.from_pretrained(REPO)
    t0 = time.time()
    pipe.load_components(names=["text_encoder", "tokenizer", "processor", "vae", "audio_vae", DIT[trunk], "scheduler",
                                "audio_scheduler"], dtype=torch.bfloat16)
    say(f"loaded in {time.time() - t0:.0f} s; config {dict((k, v) for k, v in pipe.config.items() if not isinstance(v, (list, tuple)))}")
    dit = getattr(pipe, DIT[trunk])
    # Turbo, as alibaba-pai's predict scripts apply it.
    sys.path.insert(0, os.path.dirname(hf_hub_download(ACC, "minimax_h3_pdd.py")))
    from minimax_h3_pdd import apply_pdd_lora  # noqa: E402

    nfe = apply_pdd_lora(dit, hf_hub_download(ACC, LORA[trunk]), pipe.scheduler.shift, pipe.audio_scheduler.shift)
    say(f"PDD nfe {nfe}, shifts {pipe.scheduler.shift} / {pipe.audio_scheduler.shift}")
    # The 66 GB DiT and the 67 GB text encoder stream from host RAM, a group at a time; the VAEs sit on the card.
    disk = None if available > 170 else os.path.join(out, "offload-te")
    apply_group_offloading(pipe.text_encoder, onload_device=dev, offload_type="leaf_level", use_stream=True,
                           low_cpu_mem_usage=True, offload_to_disk_path=disk)
    dit.enable_group_offload(onload_device=dev, offload_type="block_level", num_blocks_per_group=1, use_stream=True,
                             low_cpu_mem_usage=True)
    pipe.vae.to(dev)
    pipe.audio_vae.to(dev)
    for case in spec["cases"]:
        where = os.path.join(out, case["name"])
        os.makedirs(where, exist_ok=True)
        try:
            run(pipe, trunk, case, nfe, where)
        except Exception:
            open(os.path.join(where, "error.txt"), "w").write(traceback.format_exc())
            say(f"{case['name']}: FAILED\n{traceback.format_exc()}")
        gc.collect()
        torch.cuda.empty_cache()


def run(pipe, trunk, case, nfe, where):
    seed = random.Random(int(case["seed"])).getrandbits(63)  # what `cozy_runtime`'s request view hands Diffusers
    generator = torch.Generator().manual_seed(seed)
    kwargs = dict(prompt=case["prompt"], num_frames=int(case["frames"]), num_inference_steps=nfe + 1, generator=generator,
                  output_type="np")
    if trunk == "ref2va":
        edge = int(case.get("short_edge", 2048))
        pipe.update_components(reference_image_short_edge=edge)
        kwargs["references"] = [MiniMaxH3ImageReference(image=Image.open(p).convert("RGB")) for p in case["references"]]
        kwargs.update(height=int(case["height"]), width=int(case["width"]))
        say(f"{case['name']}: reference_image_short_edge {pipe.config.reference_image_short_edge}")
    elif case.get("image"):
        kwargs["image"] = Image.open(case["image"]).convert("RGB")
    else:
        kwargs.update(height=int(case["height"]), width=int(case["width"]))
    open(os.path.join(where, "prompt.txt"), "w").write(case["prompt"])
    torch.cuda.reset_peak_memory_stats()
    t0 = time.time()
    with torch.no_grad():
        state = pipe(**kwargs)
    took = time.time() - t0
    latents = state.get("latents")
    np.save(os.path.join(where, "latents.npy"), latents.float().cpu().numpy())
    videos = state.get("videos")
    frames = np.asarray(videos[0])
    np.save(os.path.join(where, "frames.npy"), (frames * 255.0).round().clip(0, 255).astype(np.uint8))
    audio = state.get("audio")
    np.save(os.path.join(where, "audio.npy"), audio[0].float().cpu().numpy())
    open(os.path.join(where, "rate.txt"), "w").write(str(int(state.get("sampling_rate"))))
    facts = {"seconds": round(took, 1), "seed": seed, "latents": list(latents.shape), "frames": list(frames.shape),
             "peak_gib": round(torch.cuda.max_memory_allocated() / 2**30, 1),
             "steps": [float(t) for t in state.get("timesteps")]}
    json.dump(facts, open(os.path.join(where, "done.json"), "w"))
    say(f"{case['name']}: {facts}")


if __name__ == "__main__":
    main()
