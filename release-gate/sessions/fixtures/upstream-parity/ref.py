"""se-158's upstream references: each model's own pipeline at a seed, on the pod, in a venv of its own.

    ref.py fetch                       the references' checkpoints into the Hugging Face cache
    ref.py tokens-a | tokens-m         this package tree's tokenizer constructions against upstream's loaders
    ref.py <cell> --out PNG [options]  one reference picture

Every cell draws its noise from a CPU generator at --seed, as cozy_runtime.rng does, so a reference and our run
start from the same noise. --attn math runs PyTorch's math SDPA instead of its fused kernels: the same pipeline
with only rounding moved, the floor any two runs of one model differ by.
"""

import argparse
import json
import math
import os
import sys
import time

import torch
from PIL import Image

SDXL = "stabilityai/stable-diffusion-xl-base-1.0"
ANIMA = "circlestone-labs/Anima-Base-v1.0-Diffusers"
MOSS = "OpenMOSS-Team/MOSS-Image-SFT"
SDXL_FILES = [
    "model_index.json", "scheduler/*", "tokenizer/*", "tokenizer_2/*",
    "text_encoder/config.json", "text_encoder/model.fp16.safetensors",
    "text_encoder_2/config.json", "text_encoder_2/model.fp16.safetensors",
    "unet/config.json", "unet/diffusion_pytorch_model.fp16.safetensors",
    "vae/config.json", "vae/diffusion_pytorch_model.fp16.safetensors",
]
MOSS_FILES = ["*.json", "*.py", "scheduler/*", "tokenizer/*", "text_encoder/*", "transformer/*", "vae/*"]
#: sdxl's `upscale` prompt and negative (sdxl/__init__.py UPSCALE_PROMPT, UPSCALE_NEGATIVE).
UPSCALE_PROMPT = "a high quality, highly detailed, sharp picture"
UPSCALE_NEGATIVE = "blurry, lowres, jpeg artifacts, noise, out of focus"


def say(**row: object) -> None:
    print(json.dumps(row), flush=True)


def snapshot(repo: str) -> str:
    from huggingface_hub import snapshot_download

    return snapshot_download(repo, local_files_only=True)


def attention(mode: str) -> None:
    if mode == "math":
        torch.backends.cuda.enable_flash_sdp(False)
        torch.backends.cuda.enable_mem_efficient_sdp(False)
        torch.backends.cuda.enable_cudnn_sdp(False)


def generator(seed: int) -> torch.Generator:
    return torch.Generator().manual_seed(seed)


def fetch() -> None:
    from huggingface_hub import HfApi, snapshot_download

    for repo, patterns in ((SDXL, SDXL_FILES), (ANIMA, None), (MOSS, MOSS_FILES)):
        t = time.time()
        path = snapshot_download(repo, allow_patterns=patterns)
        size = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(path) for f in fs)
        say(repo=repo, commit=HfApi().model_info(repo).sha, path=path, gb=round(size / 1e9, 2), s=round(time.time() - t))


# ------------------------------------------------------------------ SDXL: Diffusers' SDXL pipelines


def sdxl(cls: type) -> object:
    from diffusers import DPMSolverMultistepScheduler

    pipe = cls.from_pretrained(SDXL, torch_dtype=torch.float16, variant="fp16").to("cuda")
    # sdxl's DPM++ 2M Karras: Diffusers' documented recipe for it, over the checkpoint's scheduler config.
    pipe.scheduler = DPMSolverMultistepScheduler.from_config(
        pipe.scheduler.config, use_karras_sigmas=True, algorithm_type="dpmsolver++", solver_order=2
    )
    pipe.set_progress_bar_config(disable=True)
    return pipe


def sdxl_t2i(a: argparse.Namespace) -> Image.Image:
    from diffusers import StableDiffusionXLPipeline

    pipe = sdxl(StableDiffusionXLPipeline)
    # None is Diffusers' default: SDXL base's force_zeros_for_empty_prompt zeroes the negative branch.
    negative = None if a.zeros else a.negative
    return pipe(a.prompt, negative_prompt=negative, num_inference_steps=a.steps, guidance_scale=a.guidance,
                height=a.height, width=a.width, generator=generator(a.seed)).images[0]


def sdxl_i2i(a: argparse.Namespace) -> Image.Image:
    from diffusers import StableDiffusionXLImg2ImgPipeline

    pipe = sdxl(StableDiffusionXLImg2ImgPipeline)
    canvas = Image.open(a.image).convert("RGB").resize((a.width, a.height), Image.Resampling.LANCZOS)
    image: object = canvas
    if a.encode == "mode":
        # The picture's latent as sdxl encodes it (the posterior's mode), at Diffusers' fp32 upcast: the noise is then
        # the generator's first draw, as ours. `sample` is Diffusers' default (a draw from the posterior first).
        vae = pipe.vae
        vae.to(torch.float32)
        with torch.no_grad():
            pixels = pipe.image_processor.preprocess(canvas).to("cuda", torch.float32)
            image = (vae.encode(pixels).latent_dist.mode() * vae.config.scaling_factor).to(torch.float16)
        vae.to(torch.float16)
    return pipe(UPSCALE_PROMPT, negative_prompt=UPSCALE_NEGATIVE, image=image, strength=a.strength,
                num_inference_steps=a.steps, guidance_scale=a.guidance, generator=generator(a.seed)).images[0]


# ------------------------------------------------------------------ Anima: Diffusers' modular pipeline


def anima() -> object:
    from diffusers import ModularPipeline

    pipe = ModularPipeline.from_pretrained(ANIMA)
    pipe.load_components(torch_dtype=torch.bfloat16)
    pipe.to("cuda")
    return pipe


def anima_t2i(a: argparse.Namespace) -> Image.Image:
    pipe = anima()
    pipe.guider.guidance_scale = a.guidance
    images = pipe(prompt=a.prompt, negative_prompt=a.negative, num_inference_steps=a.steps, height=a.height,
                  width=a.width, generator=generator(a.seed), output="images")
    return images[0]


def anima_i2i(a: argparse.Namespace) -> Image.Image:
    pipe = anima()
    pipe.guider.guidance_scale = a.guidance
    canvas = Image.open(a.image).convert("RGB").resize((a.width, a.height), Image.Resampling.LANCZOS)
    images = pipe(prompt=a.prompt, negative_prompt=a.negative, image=canvas, strength=a.strength,
                  num_inference_steps=a.steps, height=a.height, width=a.width, generator=generator(a.seed),
                  output="images")
    return images[0]


# ------------------------------------------------------------------ MOSS-Image: upstream's pipeline_moss_image.py


def moss_t2i(a: argparse.Namespace) -> Image.Image:
    from diffusers import AutoencoderKLFlux2
    from transformers import AutoModel, AutoTokenizer

    root = snapshot(MOSS)
    sys.path.insert(0, root)
    from pipeline_moss_image import MOSSImagePipeline, MOSSImageScheduler, MOSSImageTransformer

    # The pipeline as upstream builds it (from_components_pretrained), its encoder and DiT taking turns on one card.
    tokenizer = AutoTokenizer.from_pretrained(root, subfolder="tokenizer")
    encoder = AutoModel.from_pretrained(root, subfolder="text_encoder", dtype=torch.bfloat16).to("cuda").eval()
    pipe = MOSSImagePipeline(None, MOSSImageScheduler(), encoder, tokenizer, None)
    positive, positive_mask = pipe.encode_prompt(a.prompt)
    negative, negative_mask = pipe.encode_prompt(a.negative)
    del pipe, encoder
    torch.cuda.empty_cache()
    transformer = MOSSImageTransformer.from_pretrained(root, subfolder="transformer", torch_dtype=torch.bfloat16)
    vae = AutoencoderKLFlux2.from_pretrained(root, subfolder="vae", torch_dtype=torch.float32)
    pipe = MOSSImagePipeline(transformer.to("cuda").eval(), MOSSImageScheduler(), None, None, vae.to("cuda").eval())
    pipe.set_progress_bar_config(disable=True)
    return pipe(prompt_embeds=positive, prompt_attention_mask=positive_mask, negative_prompt_embeds=negative,
                negative_prompt_attention_mask=negative_mask, height=a.height, width=a.width,
                num_inference_steps=a.steps, guidance_scale=a.guidance, generator=generator(a.seed)).images[0]


# ------------------------------------------------------------------ tokenizers: ours against upstream's loaders

PROMPTS = [
    "a red fox sitting in fresh snow at golden hour, detailed fur, photograph",
    "masterpiece, best quality, 1girl, silver hair, rain, neon city at night, (chibi:2), score_7",
    "worst quality, low quality, score_1, score_2, score_3, artist name, blurry, jpeg artifacts, chromatic aberration",
    'A sticky note on a fridge, handwritten: "Buy milk and eggs"  — 日本語のテキスト, émoji 🦊',
    "",
    " ".join(["a very long prompt"] * 60),
]


def tokens_a() -> None:
    """sdxl's CLIPTokenizer from vocab and merges (`sdxl._tokenizer`) and anima's PreTrainedTokenizerFast from
    tokenizer.json (`anima.model._tokenizer`), each against Transformers' own loader."""
    from tokenizers import Tokenizer
    from transformers import AutoTokenizer, CLIPTokenizer, PreTrainedTokenizerFast

    sdxl_root = snapshot(SDXL)
    for name, pad in (("tokenizer", "<|endoftext|>"), ("tokenizer_2", "!")):
        vocab = json.load(open(f"{sdxl_root}/{name}/vocab.json"))
        merges = [tuple(line.split(" ")) for line in open(f"{sdxl_root}/{name}/merges.txt").read().splitlines()
                  if line and not line.startswith("#version")]
        ours = CLIPTokenizer(vocab=vocab, merges=merges, errors="replace", pad_token=pad, model_max_length=77)
        theirs = CLIPTokenizer.from_pretrained(sdxl_root, subfolder=name)
        for p in PROMPTS:
            kw = dict(padding="max_length", max_length=77, truncation=True)
            a, b = ours(p, **kw).input_ids, theirs(p, **kw).input_ids
            say(check=f"sdxl {name}", prompt=p[:40], equal=a == b, **({} if a == b else {"ours": a, "theirs": b}))
    anima_root = snapshot(ANIMA)
    for folder in ("tokenizer", "t5_tokenizer"):
        config = json.load(open(f"{anima_root}/{folder}/tokenizer_config.json"))
        config.pop("tokenizer_class", None)
        if os.path.exists(f"{anima_root}/{folder}/chat_template.jinja"):
            config["chat_template"] = open(f"{anima_root}/{folder}/chat_template.jinja").read()
        ours = PreTrainedTokenizerFast(tokenizer_object=Tokenizer.from_file(f"{anima_root}/{folder}/tokenizer.json"), **config)
        theirs = AutoTokenizer.from_pretrained(anima_root, subfolder=folder)
        for p in PROMPTS:
            kw = dict(padding="longest", max_length=512, truncation=True)
            a, b = ours([p], **kw).input_ids, theirs([p], **kw).input_ids
            say(check=f"anima {folder} ({type(theirs).__name__})", prompt=p[:40], equal=a == b,
                **({} if a == b else {"ours": a, "theirs": b}))


def tokens_m() -> None:
    """moss-image's tokenizer (`moss_image.model.build_tokenizer`) against upstream's AutoTokenizer, through the chat
    template upstream's encode_prompt applies."""
    from tokenizers import Tokenizer
    from transformers import AutoTokenizer, PreTrainedTokenizerFast

    root = snapshot(MOSS)
    config = json.load(open(f"{root}/tokenizer/tokenizer_config.json"))
    for key in ("tokenizer_file", "name_or_path", "added_tokens_decoder", "tokenizer_class", "processor_class",
                "backend", "is_local", "local_files_only", "chat_template", "model_specific_special_tokens"):
        config.pop(key, None)
    ours = PreTrainedTokenizerFast(tokenizer_object=Tokenizer.from_file(f"{root}/tokenizer/tokenizer.json"),
                                   chat_template=open(f"{root}/tokenizer/chat_template.jinja").read(), **config)
    theirs = AutoTokenizer.from_pretrained(root, subfolder="tokenizer")
    for p in PROMPTS:
        texts = [t.apply_chat_template([{"role": "user", "content": p}], tokenize=False, add_generation_prompt=True,
                                       enable_thinking=True) for t in (ours, theirs)]
        a, b = (t(x, truncation=True, max_length=4096).input_ids for t, x in zip((ours, theirs), texts))
        say(check="moss tokenizer", prompt=p[:40], template_equal=texts[0] == texts[1], equal=a == b,
            **({} if a == b else {"ours": texts[0][-120:], "theirs": texts[1][-120:]}))


CELLS = {"sdxl-t2i": sdxl_t2i, "sdxl-i2i": sdxl_i2i, "anima-t2i": anima_t2i, "anima-i2i": anima_i2i, "moss-t2i": moss_t2i}


def main() -> None:
    if sys.argv[1:2] == ["fetch"]:
        return fetch()
    if sys.argv[1:2] == ["tokens-a"]:
        return tokens_a()
    if sys.argv[1:2] == ["tokens-m"]:
        return tokens_m()
    p = argparse.ArgumentParser()
    p.add_argument("cell", choices=sorted(CELLS))
    p.add_argument("--out", required=True)
    p.add_argument("--seed", type=int, required=True)
    p.add_argument("--prompt", default="")
    p.add_argument("--negative", default="")
    p.add_argument("--zeros", action="store_true", help="sdxl-t2i: Diffusers' default negative (None: zeros)")
    p.add_argument("--steps", type=int, required=True)
    p.add_argument("--guidance", type=float, required=True)
    p.add_argument("--width", type=int, default=1024)
    p.add_argument("--height", type=int, default=1024)
    p.add_argument("--image")
    p.add_argument("--strength", type=float, default=1.0)
    p.add_argument("--encode", choices=("mode", "sample"), default="mode")
    p.add_argument("--attn", choices=("sdpa", "math"), default="sdpa")
    a = p.parse_args()
    attention(a.attn)
    t = time.time()
    with torch.inference_mode():
        image = CELLS[a.cell](a)
    image.save(a.out)
    say(cell=a.cell, out=a.out, size=f"{image.width}x{image.height}", s=round(time.time() - t, 1),
        peak_gib=round(torch.cuda.max_memory_allocated() / 2**30, 2), torch=torch.__version__)


if __name__ == "__main__":
    main()
