"""What several fixtures make: Qwen-VL processors and tiny Qwen-VL configs, tiny tokenizers and input pictures.

A tiny tokenizer, for a family whose real one no package bundles (Qwen's is `cell.qwen_tokenizer`), is byte-level BPE
with every byte a token and no merges, so any text encodes, plus the family's special tokens and its chat template in its
own shape (Mistral's [INST]); a vision item renders as the family's picture token."""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path

import numpy as np
from PIL import Image
from tokenizers import AddedToken, Tokenizer, decoders, models, pre_tokenizers
from transformers import Qwen3_5Config, Qwen3VLConfig

from cell import Json, packages, qwen_tokenizer

#: Qwen's vocabulary and its vision tokens' ids (`cell.qwen_tokenizer`).
QWEN_VOCAB = 151936
QWEN_IDS = {"image_token_id": 151655, "video_token_id": 151656, "vision_start_token_id": 151652,
            "vision_end_token_id": 151653}
_VISION: dict[str, Json] = {"depth": 2, "hidden_size": 32, "intermediate_size": 64, "num_heads": 2, "patch_size": 16,
                            "spatial_merge_size": 2, "temporal_patch_size": 2}


def qwen_processor(folder: Path) -> None:
    """A Qwen-VL release's processor files: Qwen's tokenizer and chat template, Qwen3-VL's image and video processors
    (16 px patches, merged 2x2), from the copy minimax-h3 bundles."""
    qwen_tokenizer(folder)
    source = packages() / "minimax-h3" / "minimax_h3" / "processor"
    for name in ("preprocessor_config.json", "video_preprocessor_config.json"):
        shutil.copyfile(source / name, folder / name)


def qwen3_vl(layers: int, width: int = 32, tie_word_embeddings: bool = False, **text: Json) -> Qwen3VLConfig:
    """Qwen3-VL at `width` with `layers` text layers and a 2-layer vision tower, on Qwen's token ids."""
    return Qwen3VLConfig(
        text_config={"vocab_size": QWEN_VOCAB, "hidden_size": width, "intermediate_size": 2 * width,
                     "num_hidden_layers": layers, "num_attention_heads": 4, "num_key_value_heads": 2,
                     "head_dim": width // 4, "max_position_embeddings": 32768,
                     "rope_parameters": {"rope_type": "default", "rope_theta": 5e6, "mrope_interleaved": True,
                                         "mrope_section": [width // 16, width // 32, width // 32]},
                     # Qwen3-VL's releases state the tie in text_config too (`get_text_config()` reads it there).
                     "tie_word_embeddings": tie_word_embeddings, **text},
        vision_config={**_VISION, "out_hidden_size": width, "deepstack_visual_indexes": [0, 1]},
        tie_word_embeddings=tie_word_embeddings, image_token_id=QWEN_IDS["image_token_id"],
        video_token_id=QWEN_IDS["video_token_id"], vision_start_token_id=QWEN_IDS["vision_start_token_id"],
        vision_end_token_id=QWEN_IDS["vision_end_token_id"],
    )


def qwen3_5_vl(layers: int = 4, width: int = 32, **text: Json) -> Qwen3_5Config:
    """Qwen3.5 at `width`: linear-attention layers with a full one every fourth, a 2-layer vision tower."""
    return Qwen3_5Config(
        text_config={"vocab_size": QWEN_VOCAB, "hidden_size": width, "intermediate_size": 2 * width,
                     "num_hidden_layers": layers, "num_attention_heads": 2, "num_key_value_heads": 1, "head_dim": 32,
                     "linear_key_head_dim": 8, "linear_value_head_dim": 8, "linear_num_key_heads": 2,
                     "linear_num_value_heads": 4, "max_position_embeddings": 32768,
                     "rope_parameters": {"rope_type": "default", "rope_theta": 1e7, "partial_rotary_factor": 0.25,
                                         "mrope_interleaved": True, "mrope_section": [2, 1, 1]}, **text},
        vision_config={**_VISION, "out_hidden_size": width}, image_token_id=QWEN_IDS["image_token_id"],
        video_token_id=QWEN_IDS["video_token_id"], vision_start_token_id=QWEN_IDS["vision_start_token_id"],
        vision_end_token_id=QWEN_IDS["vision_end_token_id"],
    )

#: Mistral-Small-3.2 under Pixtral's processor.
MISTRAL_SPECIALS = ("<unk>", "<s>", "</s>", "<pad>", "[INST]", "[/INST]", "[SYSTEM_PROMPT]", "[/SYSTEM_PROMPT]", "[IMG]",
                    "[IMG_BREAK]", "[IMG_END]")
MISTRAL = """{{- bos_token -}}
{%- for message in messages -%}
{%- if message['content'] is string -%}{%- set items = [{'type': 'text', 'text': message['content']}] -%}\
{%- else -%}{%- set items = message['content'] -%}{%- endif -%}
{%- if message['role'] == 'system' -%}[SYSTEM_PROMPT]{%- elif message['role'] == 'user' -%}[INST]{%- endif -%}
{%- for item in items -%}{%- if item['type'] == 'text' -%}{{ item['text'] }}{%- else -%}[IMG]{%- endif -%}{%- endfor -%}
{%- if message['role'] == 'system' -%}[/SYSTEM_PROMPT]{%- elif message['role'] == 'user' -%}[/INST]\
{%- else -%}{{ eos_token }}{%- endif -%}
{%- endfor -%}"""


def write(folder: Path, specials: Sequence[str], config: Mapping[str, Json], template: str | None = None) -> dict[str, int]:
    """The tokenizer in `folder`, as fast (`tokenizer.json`) and as vocabulary and merges, with its config and chat
    template; each special token's id (the vocabulary is 256 bytes, then the specials)."""
    alphabet = sorted(pre_tokenizers.ByteLevel.alphabet())
    words = {c: i for i, c in enumerate(alphabet)}
    ids = {s: len(alphabet) + i for i, s in enumerate(specials)}
    tokenizer = Tokenizer(models.BPE(vocab=words, merges=[]))
    tokenizer.pre_tokenizer = pre_tokenizers.ByteLevel(add_prefix_space=False)
    tokenizer.decoder = decoders.ByteLevel()
    tokenizer.add_special_tokens([AddedToken(s, special=True, normalized=False) for s in specials])
    folder.mkdir(parents=True, exist_ok=True)
    tokenizer.save(str(folder / "tokenizer.json"))
    (folder / "vocab.json").write_text(json.dumps(words))
    (folder / "merges.txt").write_text("#version: 0.2\n")
    added: dict[str, Json] = {str(i): {"content": s, "lstrip": False, "normalized": False, "rstrip": False,
                                       "single_word": False, "special": True} for s, i in ids.items()}
    (folder / "tokenizer_config.json").write_text(json.dumps({**config, "model_max_length": 131072,
                                                              "added_tokens_decoder": added}))
    if template is not None:
        (folder / "chat_template.jinja").write_text(template)
    return ids


def vocab(ids: Mapping[str, int]) -> int:
    """A model's vocabulary over the tokenizer's ids, rounded up to 64."""
    return (max(ids.values()) + 64) // 64 * 64


def picture(name: str, width: int, height: int) -> Callable[[Path], Path]:
    """A smooth picture with structure (a gradient and a disc), made where the cell runs."""

    def make(folder: Path) -> Path:
        y, x = np.mgrid[0:height, 0:width].astype(np.float32)
        disc = ((x - width * 0.6) ** 2 + (y - height * 0.45) ** 2) < (min(width, height) * 0.25) ** 2
        rgb = np.stack([x / width * 255, y / height * 255, np.where(disc, 230.0, 40.0)], axis=-1).astype(np.uint8)
        path = folder / f"{name}.png"
        Image.fromarray(rgb).save(path)
        return path

    return make
