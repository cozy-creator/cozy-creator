"""Stand-ins for the models whose slots `slot_choices` (commands.sh) asks about (cl-943): tiny checkpoints with production's
components, classes and run files, so what a slot offers is judged as production's headers are.

    python3 slot_models.py OUT   writes one .cozytensors per release under OUT and prints `<file> <org/name> <release> <page>` per line
"""
import json
import shutil
import sys
from pathlib import Path

import tensorfs

# A component: its name, class (library, version, class), config (JSON), tensors' shapes and run files.
Part = tuple[str, tuple[str, str, str], str, dict[str, list[int]], dict[str, str]]

QWEN35 = ("transformers", "4.57.0", "Qwen3_5ForConditionalGeneration")
TOKENIZER = {
    "tokenizer_config.json": json.dumps({"tokenizer_class": "Qwen2Tokenizer", "eos_token": "<|im_end|>"}),
    "tokenizer.json": json.dumps({"version": "1.0", "model": {"type": "BPE", "vocab": {"a": 0}, "merges": []}}),
    "chat_template.jinja": "{% for m in messages %}{{ m.content }}{% endfor %}",
}
VOCABULARY = {"vocab.json": json.dumps({"a": 0}), "merges.txt": "#version: 0.2\n"}
IMAGES = {"preprocessor_config.json": json.dumps({"image_processor_type": "Qwen2VLImageProcessor"})}
VIDEOS = {"video_preprocessor_config.json": json.dumps({"video_processor_type": "Qwen3VLVideoProcessor"})}


def write(path: Path, parts: list[Part]) -> None:
    """A portable checkpoint of BF16 zeros."""
    path.parent.mkdir(parents=True, exist_ok=True)
    cache = path.with_suffix(".cache")
    writer = tensorfs.Writer(path, cache_dir=cache)
    for name, cls, config, tensors, files in parts:
        component = writer.component(name, cls=cls, config=json.loads(config))
        for key, shape in tensors.items():
            component.put(key, bytes(2 * shape[0] * shape[1]), dtype="BF16", shape=shape)
        for file, text in files.items():
            component.asset(file, text.encode())
    writer.commit().close()
    shutil.rmtree(cache)


def lm(cls: tuple[str, str, str], width: int, held: tuple[str, ...], files: dict[str, str]) -> list[Part]:
    """A Qwen3.5-class LM, its one component `model`: of its vision tower, language model (and head) and multi-token-prediction
    head, those `held`."""
    shapes = {
        "vision": {"model.visual.blocks.0.attn.qkv.weight": [3 * width, width]},
        "language": {"model.language_model.layers.0.mlp.up_proj.weight": [2 * width, width], "lm_head.weight": [16, width]},
        "mtp": {"mtp.layers.0.mlp.up_proj.weight": [2 * width, width], "mtp.fc.weight": [width, 2 * width]},
    }
    tensors = {key: shape for part in held for key, shape in shapes[part].items()}
    config = json.dumps({"architectures": [cls[2]], "model_type": "qwen3_5", "text_config": {"hidden_size": width}})
    return [("model", cls, config, tensors, files)]


def h3(processor: bool) -> list[Part]:
    """MiniMax-H3's five components; its text encoder's processor only when `processor`."""
    parts: list[Part] = []
    for name, cls in [("transformer", "MiniMaxH3Transformer3DModel"), ("transformer_ref", "MiniMaxH3Transformer3DModel"),
                      ("vae", "AutoencoderKLMiniMaxH3"), ("audio_vae", "AutoencoderKLMiniMaxH3Audio")]:
        plan = {"timestep-plan.json": json.dumps({"steps": [1000, 500]})} if name.startswith("transformer") else {}
        parts.append((name, ("diffusers", "0.37.0", cls), json.dumps({"_class_name": cls, "width": 4}),
                      {"blocks.0.attn.qkv.weight": [12, 4], "proj_out.linear.weight": [4, 4]}, plan))
    files = TOKENIZER | VOCABULARY | IMAGES | VIDEOS
    held = {f"processor/{file}": text for file, text in files.items() if file != "tokenizer.json"} if processor else {}
    parts.append(("text_encoder", ("transformers", "4.57.0", "Qwen3VLForConditionalGeneration"), json.dumps({"architectures": ["Qwen3VLForConditionalGeneration"]}),
                  {"model.language_model.layers.0.mlp.up_proj.weight": [8, 4], "model.visual.blocks.0.attn.qkv.weight": [12, 4]}, held))
    return parts


def main() -> None:
    out = Path(sys.argv[1])
    enhancer = TOKENIZER | {"processor_config.json": json.dumps({"image_processor": {"image_processor_type": "Qwen2VLImageProcessor"}}),
                            "generation_config.json": json.dumps({"top_p": 0.8}), "system_prompt.txt": "Rewrite the request."}
    qwen = TOKENIZER | VOCABULARY | IMAGES | VIDEOS | {"generation_config.json": json.dumps({"top_p": 0.95})}
    upsampler = TOKENIZER | IMAGES | VIDEOS | {"system_prompts.json": json.dumps({"t2i": "Describe."})}
    moe = ("transformers", "4.57.0", "Qwen3_5MoeForConditionalGeneration")
    made: list[tuple[str, str, str, list[Part]]] = [
        # Qwen Image's prompt enhancers (its rewriter's and expander's defaults), Qwen3.6-27B at another size with run files
        # spelled otherwise, Qwen3.6-35B-A3B of another class, and Qwen3.5-9B's head-only lane.
        ("pe-i2i", "darrellbest/qwen-image-2.1-pe-i2i-abliterated", "0.1.0", lm(QWEN35, 4, ("vision", "language"), enhancer)),
        ("pe-t2i", "pottokao/qwen-image-2.1-pe-t2i-abliterated", "0.1.0", lm(QWEN35, 4, ("vision", "language"), enhancer)),
        ("qwen3.6-27b", "qwen/qwen3.6-27b", "0.1.0", lm(QWEN35, 8, ("vision", "language", "mtp"), qwen)),
        ("qwen3.6-35b-a3b", "qwen/qwen3.6-35b-a3b", "0.1.0", lm(moe, 8, ("vision", "language", "mtp"), qwen)),
        ("qwen3.5-9b-mtp", "qwen/qwen3.5-9b-mtp", "0.2.0", lm(QWEN35, 4, ("mtp",), qwen)),
        # Klein's upsampler (its default) and BFL's.
        ("klein-upsampler-abliterated", "atomicchat/flux-3-klein-upsampler-abliterated", "0.1.0", lm(QWEN35, 4, ("vision", "language"), upsampler)),
        ("klein-upsampler", "black-forest-labs/flux-3-klein-upsampler", "0.2.0", lm(QWEN35, 4, ("vision", "language"), upsampler)),
        # H3's default; a fine-tune whose first release lacks the text encoder's processor and whose next carries it; one that never does.
        ("minimax-h3", "minimaxai/minimax-h3", "1.0.0-rc.8", h3(True)),
        ("pinkcherry-0.1.0", "sexgod1979/pinkcherry", "0.1.0", h3(False)),
        ("pinkcherry-0.1.1", "sexgod1979/pinkcherry", "0.1.1", h3(True)),
        ("eros-max", "tenstrip/eros-max", "0.1.0", h3(False)),
    ]
    for name, model, release, parts in made:
        path = out / f"{name}.cozytensors"
        write(path, parts)
        print(path, model, release, f"https://huggingface.co/{model}")


if __name__ == "__main__":
    main()
