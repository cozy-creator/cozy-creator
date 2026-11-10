"""Tiny HunyuanImage 3.0: the package's own `HunyuanImage3ForCausalMM` (the MoE decoder with its VAE, SigLIP2 tower,
aligner and patch UNets) from Tencent's release config with every width shrunk, as the one-component checkpoint the
package loads, and a tokenizer with Tencent's 2,151 special tokens at their release ids (127957 on) over a byte-level
vocabulary. Instruct and Instruct-Distil differ only in `cfg_distilled`/`use_meanflow`, as the releases do."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import tensorfs
import torch
from hunyuan_image_3.tencent.configuration_hunyuan_image_3 import HunyuanImage3Config
from hunyuan_image_3.tencent.modeling_hunyuan_image_3 import HunyuanImage3ForCausalMM
from PIL import Image
from tokenizers import AddedToken, Regex, Tokenizer, decoders, models as bpe, pre_tokenizers

from cell import Cell, Json, module, quantize, size

INSTRUCT, DISTIL = "tencent/hunyuan-image-3-instruct@0.1.1", "tencent/hunyuan-image-3-instruct-distil@0.1.1"
#: Instruct-Distil whose thought ends at once (`_terse`), for the thinking cell.
TERSE = "tiny/hunyuan-image-3-terse"
FIRST_SPECIAL = 127957
#: Tencent's config.json (HunyuanImage-3.0-Instruct), its widths shrunk: 64 wide, 2 layers of 4 experts (top 2), a
#: 1-layer SigLIP2 of 32, the VAE at 32-64 channels (its GroupNorms take 32 groups). Ids, ratios, rope, routing and
#: the VAE's 16x stay the release's.
CONFIG: dict[str, Json] = {
    "add_classification_head": False, "anyres_pooling_size": 2, "anyres_vit_max_image_size": None,
    "anyres_vit_two_views": False, "attention_bias": False, "attention_dropout": 0.0, "attention_head_dim": 16,
    "bos_token_id": 127958, "cla_share_factor": 2, "class_num": 0, "dense_list": [4096, 0], "eod_token_id": 3,
    "eos_token_id": 127957, "group_limited_greedy": False, "hidden_act": "silu", "hidden_size": 64, "im_end_id": 128001,
    "im_newline_id": 11, "im_start_id": 128000, "image_token_id": 128006, "initializer_range": 0.02,
    "intermediate_size": 64, "kv_lora_rank": None, "mask_init_id": 12, "max_position_embeddings": 22800, "mlp_bias": False,
    "model_type": "hunyuan_image_3_moe", "moe_drop_tokens": False, "moe_intermediate_size": [32, 32],
    "moe_layer_num_skipped": 0, "moe_random_routing_dropped_token": False, "moe_topk": [2, 2], "n_group": False,
    "norm_topk_prob": True, "norm_type": "rms", "num_attention_heads": 4, "num_experts": 4, "num_hidden_layers": 2,
    "num_key_value_heads": 2, "num_media_embeds": 257, "num_shared_expert": [1, 1], "pad_id": 128009,
    "pad_token_id": 128009, "pool_type": "last", "position_embedding_xdrope": False, "pretraining_tp": 1,
    "q_lora_rank": None, "qk_nope_head_dim": None, "qk_rope_head_dim": None, "rms_norm_eps": 1e-05,
    "rope_scaling": {"alpha": 1.0, "beta_fast": 32, "beta_slow": 1, "factor": 1.0, "mscale": 1.0, "mscale_all_dim": 1.0,
                     "type": "custom"},
    "rope_theta": 10000.0, "routed_scaling_factor": False, "skip_cls_token": False, "text_end_id": 7, "text_start_id": 6,
    "tie_word_embeddings": False, "topk_group": False, "use_cache": True, "use_cla": False, "use_mixed_mlp_moe": True,
    "use_mla": False, "use_qk_norm": True, "use_rotary_pos_emb": True, "v_head_dim": None, "video_end_id": 10,
    "video_start_id": 9, "vit_add_patchemb_bias": False, "vit_input_resolution": 224, "vit_mapping_type": "resampler",
    "vit_norm_type": "fused", "vit_patch": 1, "vit_path": None, "vit_remove_prenorm": False, "vit_token": 64,
    "vit_type": "siglip2-so400m-patch16-naflex", "vit_used_rms_norm": False, "vocab_size": 133120,
    "xdrope_section": None, "head_dim": 16, "rope_type": "2d", "vae_downsample_factor": [16, 16],
    "vit_downsample_factor": [16, 16], "cond_token_attn_type": "joint_full", "cond_image_type": "vae_vit",
    "vae_type": "hunyuan-image-vae-v1", "vae_dtype": "float32", "vae_autocast_dtype": "float16",
    "vae": {"_class_name": "AutoencoderKLConv3D", "block_out_channels": [32, 32, 64, 64, 64], "in_channels": 3,
            "out_channels": 3, "latent_channels": 32, "layers_per_block": 1, "ffactor_spatial": 16, "ffactor_temporal": 4,
            "sample_size": 384, "sample_tsize": 96, "downsample_match_channel": True, "upsample_match_channel": True,
            "scaling_factor": 0.562679178327931},
    "vit": {"_attn_implementation": "sdpa", "attention_dropout": 0.0, "hidden_act": "gelu_pytorch_tanh", "hidden_size": 32,
            "intermediate_size": 64, "layer_norm_eps": 1e-06, "num_attention_heads": 2, "num_channels": 3,
            "num_hidden_layers": 1, "num_patches": 256, "patch_size": 16, "torch_dtype": "float32",
            "output_attentions": False, "output_hidden_states": False, "use_return_dict": True},
    "vit_processor": {"do_convert_rgb": None, "do_normalize": True, "do_rescale": True, "do_resize": True,
                      "image_mean": [0.5, 0.5, 0.5], "image_processor_type": "Siglip2ImageProcessorFast",
                      "image_std": [0.5, 0.5, 0.5], "max_num_patches": 1024, "patch_size": 16,
                      "processor_class": "Siglip2Processor", "resample": 2, "rescale_factor": 0.00392156862745098},
    "vit_aligner": {"projector_type": "mlp_gelu", "input_dim": 32, "n_embed": 64, "depth": 2, "torch_dtype": "float32"},
    "patch_embed_hidden_dim": 32,
}
_NAMED = ("<|endoftext|>", "<|startoftext|>", "<|bos|>", "<|eos|>", "<|pad|>")
_MEDIA = ("<boi>", "<eoi>", "<boa>", "<eoa>", "<bov>", "<eov>", "<img>", "<audio>", "<video>", "<pad>", "<cfg>", "<iw>",
          "<ih>", "<bot>", "<eot>", "<text>", "<mask>", "<timestep>", "<recaption>", "</recaption>", "<bof>", "<eof>",
          "<face>", "<think>", "</think>", "<answer>", "</answer>", "<und_boi>", "<und_eoi>", "<src_boi>", "<src_eoi>",
          "<gen_boi>", "<gen_eoi>", "<gen_img>")
#: Tencent's pre-tokenizer split, before its byte-level mapping.
_SPLIT = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+"
_SIZES = (256, 512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192)
_NAMES = {torch.bfloat16: "BF16", torch.float16: "F16", torch.float32: "F32", torch.int64: "I64", torch.int32: "I32",
          torch.bool: "BOOL"}


def specials() -> list[str]:
    """Tencent's added tokens in their release order (ids 127957 to 130107)."""
    return [*_NAMED, *(f"<|extra_{i}|>" for i in range(38)), *_MEDIA, *(f"<img_size_{s}>" for s in _SIZES),
            *(f"<img_ratio_{i}>" for i in range(33)), "<joint_img_sep>", "<ref>", "</ref>", "<quad>", "</quad>",
            *(f"<pos_x_{i}>" for i in range(1000)), *(f"<pos_y_{i}>" for i in range(1000)), "<guidance>",
            *(f"<relation_{i}>" for i in range(10)), *(f"</relation_{i}>" for i in range(10)),
            *(f"<img_ratio_{i}>" for i in range(33, 37)), "<timestep_r>"]


def tokenizer() -> dict[str, bytes]:
    """`tokenizer.json` and `tokenizer_config.json`: byte-level BPE (each byte a token, no merges, the rest of the
    127,957 ordinary ids unused) with Tencent's pre-tokenizer split, then the specials at their release ids."""
    alphabet = sorted(pre_tokenizers.ByteLevel.alphabet())
    vocab = {c: i for i, c in enumerate(alphabet)} | {f"<unused_{i}>": i for i in range(len(alphabet), FIRST_SPECIAL)}
    made = Tokenizer(bpe.BPE(vocab=vocab, merges=[]))
    made.pre_tokenizer = pre_tokenizers.Sequence([
        pre_tokenizers.Split(pattern=Regex(_SPLIT), behavior="isolated"),
        pre_tokenizers.ByteLevel(add_prefix_space=False, use_regex=False)])
    made.decoder = decoders.ByteLevel()
    made.add_special_tokens([AddedToken(s, special=True, normalized=False) for s in specials()])
    assert made.token_to_id("<timestep_r>") == 130107 and made.token_to_id("<|endoftext|>") == FIRST_SPECIAL
    config = {"bos_token": "<|startoftext|>", "clean_up_tokenization_spaces": True, "eos_token": "<|endoftext|>",
              "model_max_length": 1048576, "pad_token": "<pad>", "tokenizer_class": "PreTrainedTokenizerFast"}
    return {"tokenizer.json": made.to_str().encode(), "tokenizer_config.json": json.dumps(config).encode()}


def _terse(model: torch.nn.Module) -> None:
    """A random LM never ends its thought within the 22,800 tokens the image stage reads (2,048 sampled byte-level
    tokens decode and re-encode to some 37,000): here the decoder layers add nothing to the text stream and every token
    embeds alike, so the last hidden state is one vector, and the end-of-text row of the head is that vector: the think
    stage ends on its first token."""
    with torch.no_grad():
        for name, parameter in model.named_parameters():
            if name.startswith("model.layers.") and name.endswith(("o_proj.weight", "down_proj.weight")):
                parameter.zero_()
        embedding = model.get_parameter("model.wte.weight")
        embedding.fill_(0.1)
        model.get_parameter("lm_head.weight")[FIRST_SPECIAL].fill_(1.0)


def _checkpoint(store: Path, distilled: bool, seed: int, assets: dict[str, bytes], terse: bool = False) -> str:
    config = {**CONFIG, "cfg_distilled": distilled, "use_meanflow": distilled}
    torch.manual_seed(seed)
    model = module(HunyuanImage3ForCausalMM(HunyuanImage3Config(**config))).to(torch.bfloat16)
    if terse:
        _terse(model)
    generator = torch.Generator().manual_seed(seed)
    writer = tensorfs.Writer(cache_dir=store)
    component = writer.component("model", cls=None, config=config)
    for key, tensor in model.state_dict().items():
        data = tensor.detach()
        if data.is_floating_point() and not bool(torch.isfinite(data).all()):
            std = math.prod(data.shape[1:]) ** -0.5 if data.dim() > 1 else 0.02
            data = (torch.randn(data.shape, generator=generator) * std).to(data.dtype)
        data = data.contiguous()
        component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=_NAMES[data.dtype],
                      shape=list(data.shape))
    for name, body in assets.items():
        component.asset(name, body)
    return writer.commit().id


def models(root: Path, store: Path) -> dict[str, Path | str]:
    assets = tokenizer()
    return {INSTRUCT: _checkpoint(store, False, 0, assets), DISTIL: _checkpoint(store, True, 1, assets),
            TERSE: _checkpoint(store, True, 2, assets, terse=True)}


def fox(folder: Path) -> Path:
    y, x = np.mgrid[0:256, 0:320].astype(np.float32)
    path = folder / "fox.png"
    Image.fromarray(np.stack([x * 0.8, y, np.where((x - 160) ** 2 + (y - 128) ** 2 < 4000, 220.0, 60.0)], -1)
                    .astype(np.uint8)).save(path)
    return path


def _prompt(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


_DIRECT: dict[str, Json] = {"seed": 7, "aspect_ratio": "1:1", "prompt_upsampling": False}

CELLS: list[Cell] = [
    {"entry": "generate_turbo", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_DIRECT},
     "check": size(1024, 1024)},
    {"entry": "generate_turbo", "name": "thinking", "payload": {"prompt": _prompt("a red fox"), "seed": 7},
     "models": {"generate_turbo.models.model": TERSE}},
    {"entry": "generate_turbo", "name": "edit", "payload": {"prompt": _prompt("give <fox> a hat", "fox"), **_DIRECT},
     "files": {"prompt.references.0.file": fox}},
    {"entry": "generate", "name": "text", "payload": {"prompt": _prompt("a red fox in fresh snow"), **_DIRECT, "steps": 2},
     "check": size(1024, 1024)},
    {"entry": "image", "name": "tool", "payload": {"instructions": _prompt("a red fox"), "seed": 7,
                                                   "prompt_upsampling": False}},
    # bf16 is a derivation (the VAE's conv operands in float16); int8, nvfp4 and w4a8 take int8.convrot's 256-column blocks.
    quantize(INSTRUCT, "bf16", "hunyuan-image-3", "hunyuan-image-3@0.6.1"),
]
