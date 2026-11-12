"""Stand-ins for the models whose slots `slot_choices` (commands.sh) asks about (cl-943): tiny checkpoints with production's
components, classes and run files, so what a slot offers is judged as production's headers are.

    python3 slot_models.py OUT   writes one folder per release under OUT and prints `<folder> <org/name> <release> <page>` per line
"""
import json
import struct
import sys
from pathlib import Path

QWEN35 = "Qwen3_5ForConditionalGeneration"
TOKENIZER = {
    "tokenizer_config.json": {"tokenizer_class": "Qwen2Tokenizer", "eos_token": "<|im_end|>"},
    "tokenizer.json": {"version": "1.0", "model": {"type": "BPE", "vocab": {"a": 0}, "merges": []}},
    "chat_template.jinja": "{% for m in messages %}{{ m.content }}{% endfor %}",
}


def weights(path: Path, shapes: dict[str, list[int]]) -> None:
    """A safetensors file of BF16 zeros."""
    header, at = {}, 0
    for key, shape in shapes.items():
        size = 2 * shape[0] * shape[1]
        header[key] = {"dtype": "BF16", "shape": shape, "data_offsets": [at, at + size]}
        at += size
    raw = json.dumps(header).encode()
    raw += b" " * (-len(raw) % 8)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(struct.pack("<Q", len(raw)) + raw + bytes(at))


def files(folder: Path, named: dict[str, object]) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    for name, body in named.items():
        (folder / name).parent.mkdir(parents=True, exist_ok=True)
        (folder / name).write_text(body if isinstance(body, str) else json.dumps(body))


def lm(folder: Path, cls: str, width: int, parts: tuple[str, ...], run: dict[str, object]) -> None:
    """A Qwen3.5-class LM: its vision tower, language model and head, a multi-token-prediction head among `parts`."""
    shapes = {
        "vision": {"model.visual.blocks.0.attn.qkv.weight": [3 * width, width]},
        "language": {"model.language_model.layers.0.mlp.up_proj.weight": [2 * width, width], "lm_head.weight": [16, width]},
        "mtp": {"mtp.layers.0.mlp.up_proj.weight": [2 * width, width], "mtp.fc.weight": [width, 2 * width]},
    }
    weights(folder / "model.safetensors", {key: shape for part in parts for key, shape in shapes[part].items()})
    files(folder, {"config.json": {"architectures": [cls], "model_type": "qwen3_5", "text_config": {"hidden_size": width}}, **run})


def h3(folder: Path, processor: bool) -> None:
    """MiniMax-H3's five components; its text encoder's processor only when `processor`."""
    diffusers = {"transformer": "MiniMaxH3Transformer3DModel", "transformer_ref": "MiniMaxH3Transformer3DModel", "vae": "AutoencoderKLMiniMaxH3",
                 "audio_vae": "AutoencoderKLMiniMaxH3Audio"}
    index = {"_class_name": "MiniMaxH3Pipeline", "_diffusers_version": "0.37.0", "text_encoder": ["transformers", "Qwen3VLForConditionalGeneration"]}
    for name, cls in diffusers.items():
        index[name] = ["diffusers", cls]
        weights(folder / name / "diffusion_pytorch_model.safetensors", {"blocks.0.attn.qkv.weight": [12, 4], "proj_out.linear.weight": [4, 4]})
        run: dict[str, object] = {"config.json": {"_class_name": cls, "_diffusers_version": "0.37.0", "width": 4}}
        if name.startswith("transformer"):
            run["timestep-plan.json"] = {"steps": [1000, 500]}
        files(folder / name, run)
    weights(folder / "text_encoder/model.safetensors", {"model.language_model.layers.0.mlp.up_proj.weight": [8, 4], "model.visual.blocks.0.attn.qkv.weight": [12, 4]})
    run = {"config.json": {"architectures": ["Qwen3VLForConditionalGeneration"], "model_type": "qwen3_vl"}}
    if processor:
        run |= {"processor/tokenizer_config.json": TOKENIZER["tokenizer_config.json"], "processor/vocab.json": {"a": 0}, "processor/merges.txt": "#version: 0.2\n",
                "processor/preprocessor_config.json": {"image_processor_type": "Qwen2VLImageProcessor"},
                "processor/video_preprocessor_config.json": {"video_processor_type": "Qwen3VLVideoProcessor"}}
    files(folder / "text_encoder", run)
    files(folder, {"model_index.json": index})


def main() -> None:
    out = Path(sys.argv[1])
    enhancer = TOKENIZER | {"processor_config.json": {"image_processor": {"image_processor_type": "Qwen2VLImageProcessor"}}, "generation_config.json": {"top_p": 0.8},
                            "system_prompt.txt": "Rewrite the request."}
    qwen = TOKENIZER | {"vocab.json": {"a": 0}, "merges.txt": "#version: 0.2\n", "preprocessor_config.json": {"image_processor_type": "Qwen2VLImageProcessor"},
                        "video_preprocessor_config.json": {"video_processor_type": "Qwen3VLVideoProcessor"}, "generation_config.json": {"top_p": 0.95}}
    upsampler = TOKENIZER | {"preprocessor_config.json": {"image_processor_type": "Qwen2VLImageProcessor"},
                             "video_preprocessor_config.json": {"video_processor_type": "Qwen3VLVideoProcessor"}, "system_prompts.json": {"t2i": "Describe."}}
    made = [
        # Qwen Image's prompt enhancers (its rewriter's and expander's defaults), Qwen3.6-27B at another size with run files
        # spelled otherwise, Qwen3.6-35B-A3B of another class, and Qwen3.5-9B's head-only lane.
        ("pe-i2i", "darrellbest/qwen-image-2.1-pe-i2i-abliterated", "0.1.0", lambda d: lm(d, QWEN35, 4, ("vision", "language"), enhancer)),
        ("pe-t2i", "pottokao/qwen-image-2.1-pe-t2i-abliterated", "0.1.0", lambda d: lm(d, QWEN35, 4, ("vision", "language"), enhancer)),
        ("qwen3.6-27b", "qwen/qwen3.6-27b", "0.1.0", lambda d: lm(d, QWEN35, 8, ("vision", "language", "mtp"), qwen)),
        ("qwen3.6-35b-a3b", "qwen/qwen3.6-35b-a3b", "0.1.0", lambda d: lm(d, "Qwen3_5MoeForConditionalGeneration", 8, ("vision", "language", "mtp"), qwen)),
        ("qwen3.5-9b-mtp", "qwen/qwen3.5-9b-mtp", "0.2.0", lambda d: lm(d, QWEN35, 4, ("mtp",), qwen)),
        # Klein's upsampler (its default) and BFL's.
        ("klein-upsampler-abliterated", "atomicchat/flux-3-klein-upsampler-abliterated", "0.1.0", lambda d: lm(d, QWEN35, 4, ("vision", "language"), upsampler)),
        ("klein-upsampler", "black-forest-labs/flux-3-klein-upsampler", "0.2.0", lambda d: lm(d, QWEN35, 4, ("vision", "language"), upsampler)),
        # H3's default; a fine-tune whose first release lacks the text encoder's processor and whose next carries it; one that never does.
        ("minimax-h3", "minimaxai/minimax-h3", "1.0.0-rc.8", lambda d: h3(d, True)),
        ("pinkcherry-0.1.0", "sexgod1979/pinkcherry", "0.1.0", lambda d: h3(d, False)),
        ("pinkcherry-0.1.1", "sexgod1979/pinkcherry", "0.1.1", lambda d: h3(d, True)),
        ("eros-max", "tenstrip/eros-max", "0.1.0", lambda d: h3(d, False)),
    ]
    for folder, model, release, write in made:
        write(out / folder)
        print(out / folder, model, release, f"https://huggingface.co/{model}")


if __name__ == "__main__":
    main()
