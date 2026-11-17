"""The upstream references of upstream-parity.sh, run on the rental as root: each model card's or official pipeline's
own code at the pinned revisions, bf16, PyTorch SDPA, a CPU generator. `python upstream.py <group> <out> <fixtures>`.

  qwen-image  Diffusers 0.41 QwenImagePipeline (Qwen-Image-2512) and QwenImageEditPlusPipeline (Qwen-Image-Edit-2511),
              each card's settings, then each with lightx2v's 8-step Lightning LoRA and scheduler
              (ModelTC/Qwen-Image-Lightning generate_with_diffusers.py)
  pan2        Diffusers 0.43 PAN2 ModularPipeline (IFM/PAN2), t2v and i2v, 25 frames at 1248x704
  qwen3       Transformers' generate, greedy, on huihui-ai/Huihui-Qwen3.5-9B-abliterated
  birefnet    BiRefNet's model card and its handler.py (refine_foreground) at ZhengPeng7/BiRefNet e2bf8e44
"""

from __future__ import annotations

import gc
import json
import math
import os
import sys
import time
from pathlib import Path

import numpy as np
import torch
from PIL import Image

SEED = 42
QWEN_2512 = ("Qwen/Qwen-Image-2512", "25468b98e3276ca6700de15c6628e51b7de54a26")
QWEN_EDIT = ("Qwen/Qwen-Image-Edit-2511", "6f3ccc0b56e431dc6a0c2b2039706d7d26f22cb9")
LIGHTNING_T2I = ("lightx2v/Qwen-Image-2512-Lightning", "4609161e308ce82018a42c2a0c9b107e865bb751",
                 "Qwen-Image-2512-Lightning-8steps-V1.0-fp32.safetensors")
LIGHTNING_EDIT = ("lightx2v/Qwen-Image-Edit-2511-Lightning", "107d238d68b8387e32843d0b4a2e24ceb0d3ad86",
                  "Qwen-Image-Edit-2511-Lightning-8steps-V1.0-fp32.safetensors")
PAN2 = ("IFM/PAN2", "82f54c6cea3e20a9c451b61d7360016e5970503d")
QWEN3 = ("huihui-ai/Huihui-Qwen3.5-9B-abliterated", "05b9e7c9b978ba29bdb8f50a49c30e4b91183339")
BIREFNET = ("ZhengPeng7/BiRefNet", "e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4")

#: The cells' prompts, shared with upstream-parity.sh (cells.json).
CELLS = json.loads((Path(__file__).with_name("cells.json")).read_text())


def say(*words: object) -> None:
    print(time.strftime("%H:%M:%S"), *words, flush=True)


def generator() -> torch.Generator:
    return torch.Generator("cpu").manual_seed(SEED)


def free() -> None:
    gc.collect()
    torch.cuda.empty_cache()


def versions() -> dict[str, str]:
    import importlib.metadata as m

    names = ("torch", "diffusers", "transformers", "accelerate", "peft", "torchvision", "timm")
    found = {}
    for name in names:
        try:
            found[name] = m.version(name)
        except m.PackageNotFoundError:
            pass
    return found


def snapshot(repo: tuple[str, ...], patterns: list[str] | None = None) -> str:
    from huggingface_hub import snapshot_download

    return snapshot_download(repo[0], revision=repo[1], allow_patterns=patterns)


# ---- Qwen Image 1 -------------------------------------------------------------------------------------------------

LIGHTNING_SCHEDULER = {
    "base_image_seq_len": 256,
    "base_shift": math.log(3),
    "invert_sigmas": False,
    "max_image_seq_len": 8192,
    "max_shift": math.log(3),
    "num_train_timesteps": 1000,
    "shift": 1.0,
    "shift_terminal": None,
    "stochastic_sampling": False,
    "time_shift_type": "exponential",
    "use_beta_sigmas": False,
    "use_dynamic_shifting": True,
    "use_exponential_sigmas": False,
    "use_karras_sigmas": False,
}


def qwen_image(out: Path, fixtures: Path) -> None:
    from diffusers import (
        FlowMatchEulerDiscreteScheduler,
        QwenImageEditPlusPipeline,
        QwenImagePipeline,
        QwenImageTransformer2DModel,
    )
    from huggingface_hub import hf_hub_download

    cells = CELLS["qwen-image"]
    base = snapshot(QWEN_2512)
    say("Qwen-Image-2512 at", base)
    pipe = QwenImagePipeline.from_pretrained(base, torch_dtype=torch.bfloat16)
    pipe.enable_model_cpu_offload()
    t2i = cells["t2i"]
    image = pipe(prompt=t2i["prompt"], negative_prompt=t2i["negative"], width=t2i["width"], height=t2i["height"],
                 num_inference_steps=t2i["steps"], true_cfg_scale=t2i["guidance"], generator=generator()).images[0]
    image.save(out / "qwen-image-t2i.png")
    say("t2i done")
    default_scheduler = pipe.scheduler
    pipe.scheduler = FlowMatchEulerDiscreteScheduler.from_config(LIGHTNING_SCHEDULER)
    pipe.load_lora_weights(hf_hub_download(LIGHTNING_T2I[0], LIGHTNING_T2I[2], revision=LIGHTNING_T2I[1]))
    turbo = cells["t2i-turbo"]
    image = pipe(prompt=turbo["prompt"], negative_prompt=" ", width=turbo["width"], height=turbo["height"],
                 num_inference_steps=turbo["steps"], true_cfg_scale=1.0, generator=generator()).images[0]
    image.save(out / "qwen-image-t2i-turbo.png")
    say("t2i turbo done")
    pipe.unload_lora_weights()
    text_encoder, vae, tokenizer = pipe.text_encoder, pipe.vae, pipe.tokenizer
    pipe.remove_all_hooks()
    del pipe
    free()

    edit_root = snapshot(QWEN_EDIT, ["model_index.json", "transformer/*", "processor/*", "scheduler/*", "tokenizer/*"])
    say("Qwen-Image-Edit-2511 at", edit_root)
    transformer = QwenImageTransformer2DModel.from_pretrained(edit_root, subfolder="transformer", torch_dtype=torch.bfloat16)
    # The text encoder and VAE: Qwen-Image-2512's, byte for byte the edit checkpoint's (their LFS oids match).
    edit = QwenImageEditPlusPipeline.from_pretrained(
        edit_root, transformer=transformer, text_encoder=text_encoder, vae=vae, tokenizer=tokenizer,
        scheduler=default_scheduler, torch_dtype=torch.bfloat16,
    )
    edit.enable_model_cpu_offload()
    cell = cells["edit"]
    picture = Image.open(fixtures / cell["image"]).convert("RGB")
    image = edit(image=[picture], prompt=cell["prompt"], negative_prompt=cell["negative"],
                 num_inference_steps=cell["steps"], true_cfg_scale=cell["guidance"], guidance_scale=1.0,
                 generator=generator()).images[0]
    image.save(out / "qwen-image-edit.png")
    say("edit done", image.size)
    edit.scheduler = FlowMatchEulerDiscreteScheduler.from_config(LIGHTNING_SCHEDULER)
    edit.load_lora_weights(hf_hub_download(LIGHTNING_EDIT[0], LIGHTNING_EDIT[2], revision=LIGHTNING_EDIT[1]))
    cell = cells["edit-turbo"]
    image = edit(image=[picture], prompt=cell["prompt"], negative_prompt=" ", num_inference_steps=cell["steps"],
                 true_cfg_scale=1.0, guidance_scale=1.0, generator=generator()).images[0]
    image.save(out / "qwen-image-edit-turbo.png")
    say("edit turbo done", image.size)


# ---- PAN2 ---------------------------------------------------------------------------------------------------------


def pan2(out: Path, fixtures: Path) -> None:
    from diffusers import ModularPipeline

    from diffusers.modular_pipelines.pan2.decoders import PAN2VaeDecoderStep
    from diffusers.modular_pipelines.modular_pipeline import PipelineState

    # Diffusers' PAN2 docs: `ModularPipeline.from_pretrained("IFM/PAN2")`, `load_components(dtype=torch.bfloat16)`
    # (the VAE too, as the native pipeline loads it). The guardrail is a separate package, off here as in the package.
    pipe = ModularPipeline.from_pretrained(PAN2[0], revision=PAN2[1])
    bf16 = torch.bfloat16
    pipe.load_components(names=["transformer", "text_encoder", "vae", "scheduler", "tokenizer"],
                         dtype={"transformer": bf16, "text_encoder": bf16, "vae": bf16}, revision=PAN2[1])
    pipe.disable_safety_checker()
    say("dtypes: vae", pipe.vae.dtype, "text encoder", pipe.text_encoder.dtype,
        "modulation", pipe.transformer.transformer_blocks[0].norm1.linear.weight.dtype)
    # 48 GB holds the text encoder and the VAE; the 63 GiB DiT streams its blocks (Diffusers' group offload).
    pipe.text_encoder.to("cuda")
    pipe.vae.to("cuda")
    pipe.transformer.enable_group_offload(
        onload_device=torch.device("cuda"), offload_device=torch.device("cpu"), offload_type="block_level",
        num_blocks_per_group=1, use_stream=True,
    )
    say("execution device", pipe._execution_device, "guider", pipe.guider)
    assert str(pipe._execution_device).startswith("cuda"), pipe._execution_device
    examples = json.loads((fixtures / "pan2-examples.json").read_text())
    for name in ("t2v", "i2v"):
        cell = CELLS["pan2"][name]
        cell["prompt"] = examples[cell["example"]]["refined_prompt"]
        kwargs = {}
        if cell.get("image"):
            kwargs["image"] = Image.open(fixtures / cell["image"]).convert("RGB")
        made = pipe(prompt=cell["prompt"], num_frames=cell["frames"], fps=24.0, height=cell["height"],
                    width=cell["width"], num_inference_steps=cell["steps"], generator=generator(),
                    output=["latents", "videos"], output_type="np", **kwargs)
        frames = np.asarray(made["videos"])
        frames = frames[0] if frames.ndim == 5 else frames
        np.save(out / f"pan2-{name}.npy", np.rint(frames.clip(0, 1) * 255).astype(np.uint8))
        latents = made["latents"]
        np.save(out / f"pan2-{name}-latents.npy", latents.float().cpu().numpy())
        say(name, "done", frames.shape, tuple(latents.shape))
        free()
        # The same latents through the VAE at its stored F32, as the package decodes them.
        vae32 = pipe.vae.float()
        decoder = PAN2VaeDecoderStep()
        stage = decoder.init_pipeline()
        stage.disable_safety_checker()
        stage.update_components(vae=vae32)
        state = PipelineState(values={"latents": latents, "output_type": "np"})
        decoder(stage, state)
        frames = np.asarray(state.values["videos"])
        frames = frames[0] if frames.ndim == 5 else frames
        np.save(out / f"pan2-{name}-vae32.npy", np.rint(frames.clip(0, 1) * 255).astype(np.uint8))
        pipe.vae.to(torch.bfloat16)
        vae32.clear_cache()
        free()


# ---- Qwen3 --------------------------------------------------------------------------------------------------------


def qwen3(out: Path, fixtures: Path) -> None:
    from transformers import AutoModelForImageTextToText, AutoProcessor

    root = snapshot(QWEN3)
    processor = AutoProcessor.from_pretrained(root)
    model = AutoModelForImageTextToText.from_pretrained(root, dtype=torch.bfloat16, attn_implementation="sdpa").to("cuda")
    model.eval()
    # The chat's end and the configured eos: the checkpoint has no generation_config.json.
    configured = model.generation_config.eos_token_id
    stops = sorted({processor.tokenizer.eos_token_id, *(configured if isinstance(configured, list) else [configured])} - {None})
    say("stops", stops)
    answers = {}
    for name, cell in CELLS["qwen3"].items():
        content = []
        images = []
        if cell.get("image"):
            images.append(Image.open(fixtures / cell["image"]).convert("RGB"))
            content.append({"type": "image"})
        content.append({"type": "text", "text": cell["prompt"]})
        text = processor.apply_chat_template([{"role": "user", "content": content}], add_generation_prompt=True,
                                             tokenize=False, enable_thinking=False)
        inputs = processor(text=[text], images=images or None, return_tensors="pt").to("cuda")
        with torch.inference_mode():
            made = model.generate(**inputs, max_new_tokens=cell["max_tokens"], do_sample=False, eos_token_id=stops)
        tokens = made[0, inputs["input_ids"].shape[1]:].tolist()
        answers[name] = {"text": processor.decode(tokens, skip_special_tokens=True).strip(), "tokens": tokens,
                         "prompt_tokens": int(inputs["input_ids"].shape[1])}
        say(name, len(tokens), "tokens")
    (out / "qwen3.json").write_text(json.dumps(answers, ensure_ascii=False, indent=1))


# ---- BiRefNet -----------------------------------------------------------------------------------------------------


def birefnet(out: Path, fixtures: Path) -> None:
    import importlib
    import shutil

    from safetensors.torch import load_file
    from torchvision import transforms

    root = Path(snapshot(BIREFNET, ["*.py", "*.json", "model.safetensors"]))
    # birefnet.py imports its config relatively: the two files as one package, as `trust_remote_code` loads them.
    package = out.parent / "birefnet_hf"
    package.mkdir(exist_ok=True)
    for name in ("birefnet.py", "BiRefNet_config.py"):
        shutil.copy(root / name, package / name)
    (package / "__init__.py").write_text("")
    sys.path.insert(0, str(out.parent))
    module = importlib.import_module("birefnet_hf.birefnet")
    # handler.py's refine_foreground, as the HF endpoint runs it: its module above `ImagePreprocessor` (numpy, cv2, PIL).
    source = (root / "handler.py").read_text().split("class ImagePreprocessor")[0]
    space: dict[str, object] = {}
    exec(compile(source, str(root / "handler.py"), "exec"), space)
    refine_foreground = space["refine_foreground"]

    model = module.BiRefNet(bb_pretrained=False)
    state = load_file(str(root / "model.safetensors"))
    missing, unexpected = model.load_state_dict(state, strict=False)
    say("birefnet state", len(state), "missing", missing[:5], "unexpected", unexpected[:5])
    torch.set_float32_matmul_precision("high")
    model.to("cuda").eval().half()
    prepare = transforms.Compose([
        transforms.Resize((1024, 1024)),
        transforms.ToTensor(),
        transforms.Normalize([0.485, 0.456, 0.406], [0.229, 0.224, 0.225]),
    ])

    def cutout(image: Image.Image, name: str) -> tuple[Image.Image, Image.Image]:
        with torch.no_grad():
            preds = model(prepare(image).unsqueeze(0).to("cuda").half())[-1].sigmoid().cpu()
        pred_pil = transforms.ToPILImage()(preds[0].squeeze())
        mask = pred_pil.resize(image.size)  # the card's: Pillow's default filter
        foreground = refine_foreground(image, pred_pil)
        white = Image.new("RGB", image.size, "white")
        mask.save(out / f"birefnet-{name}-mask.png")
        Image.composite(image, white, mask).save(out / f"birefnet-{name}-card.png")
        Image.composite(foreground, white, mask).save(out / f"birefnet-{name}-handler.png")
        return foreground, mask

    for name in CELLS["birefnet"]["images"]:
        image = Image.open(fixtures / f"{name}.jpg").convert("RGB")
        foreground, mask = cutout(image, name)
        if name == CELLS["birefnet"]["synthetic_from"]:
            # A known cutout on green: this subject's refined colours and mask over a saturated green, and the same
            # over white (the truth a cutout of the green picture should give).
            f = np.asarray(foreground, dtype=np.float64)
            a = np.asarray(mask, dtype=np.float64)[..., None] / 255
            green = np.array([0.0, 177.0, 64.0])
            Image.fromarray(np.rint(a * f + (1 - a) * green).astype(np.uint8)).save(out / "birefnet-green.png")
            Image.fromarray(np.rint(a * f + (1 - a) * 255).astype(np.uint8)).save(out / "birefnet-green-truth.png")
            mask.save(out / "birefnet-green-truth-mask.png")
    cutout(Image.open(out / "birefnet-green.png").convert("RGB"), "green")


GROUPS = {"qwen-image": qwen_image, "pan2": pan2, "qwen3": qwen3, "birefnet": birefnet}
#: Each group's weights, fetched ahead while another group runs (`--fetch`): what each reference above reads.
FETCH = {
    "qwen-image": [(QWEN_2512, None),
                   (QWEN_EDIT, ["model_index.json", "transformer/*", "processor/*", "scheduler/*", "tokenizer/*"]),
                   (LIGHTNING_T2I, [LIGHTNING_T2I[2]]), (LIGHTNING_EDIT, [LIGHTNING_EDIT[2]])],
    "pan2": [(PAN2, ["modular_model_index.json", "transformer/*", "text_encoder/*", "vae/*", "tokenizer/*",
                     "scheduler/*"])],
    "qwen3": [(QWEN3, None)],
    "birefnet": [(BIREFNET, ["*.py", "*.json", "model.safetensors"])],
}

if __name__ == "__main__":
    group, out, fixtures = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    if "--fetch" in sys.argv:
        for repo, patterns in FETCH[group]:
            say("fetched", snapshot(repo, patterns))
        sys.exit(0)
    out.mkdir(parents=True, exist_ok=True)
    say(group, versions(), torch.cuda.get_device_name(0), os.environ.get("HF_HOME"))
    started = time.time()
    GROUPS[group](out, fixtures)
    (out / f"{group}.done").write_text(json.dumps({"seconds": round(time.time() - started, 1), "versions": versions()}))
    say(group, "finished in", round(time.time() - started), "s")
