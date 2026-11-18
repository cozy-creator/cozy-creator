"""Tiny LTX-2.5: customized Gemma4, AV DiT/connectors, conv VAE, x2 upsampler and F32 BWE vocoder.

Keep the production spatial/temporal compression and two-stage sampler; narrow channels/layers,
not requested output dimensions. Large-model quality/latency is qualified separately on H200.
"""
from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import (AutoencoderKLLTX2Audio, AutoencoderKLLTX2Video, FlowMatchEulerDiscreteScheduler,
                       LTX2LatentUpsamplerModel, LTX2TextConnectors, LTX2VideoTransformer3DModel)
from diffusers.pipelines.ltx2.vocoder import LTX2VocoderWithBWE
from transformers import Gemma4UnifiedConfig, Gemma4UnifiedForConditionalGeneration

from cell import Cell, Json, module, save
from tiny import _shared
from tiny._video import video

REF = "lightricks/ltx-2.5@0.2.0"
_SPECIALS = ("<unk>", "<pad>", "<bos>", "<eos>", "<start_of_turn>", "<end_of_turn>", "<image>")


def models(root: Path, store: Path) -> dict[str, Path | str]:
    out = root / "ltx-2.5"
    torch.manual_seed(0)
    ids = _shared.write(out / "tokenizer", _SPECIALS,
                        {"tokenizer_class": "GemmaTokenizer", "unk_token": "<unk>", "pad_token": "<pad>",
                         "bos_token": "<bos>", "eos_token": "<eos>"},
                        "{{ bos_token }}{% for m in messages %}{{ '<start_of_turn>' + m['role'] + '\\n' + m['content'] + '<end_of_turn>\\n' }}{% endfor %}{% if add_generation_prompt %}{{ '<start_of_turn>model\\n' }}{% endif %}")
    config = Gemma4UnifiedConfig(
        text_config={"vocab_size": _shared.vocab(ids), "hidden_size": 32, "intermediate_size": 64,
                     "num_hidden_layers": 2, "num_attention_heads": 1, "num_key_value_heads": 1,
                     "num_global_key_value_heads": 1, "head_dim": 8, "global_head_dim": 8,
                     "layer_types": ["sliding_attention", "full_attention"], "sliding_window": 32,
                     "max_position_embeddings": 2048, "bos_token_id": ids['<bos>'],
                     "eos_token_id": ids['<eos>'], "pad_token_id": ids['<pad>'],
                     "final_logit_softcapping": 30.0, "tie_word_embeddings": True,
                     "use_bidirectional_attention": "vision", "attention_k_eq_v": True},
        vision_config={"mm_embed_dim": 32, "output_proj_dims": 32, "mm_posemb_size": 16,
                       "patch_size": 16, "pooling_kernel_size": 1, "num_soft_tokens": 4},
        audio_config={"audio_embed_dim": 32},
        image_token_id=ids['<image>'], tie_word_embeddings=True,
    )
    setattr(config, "gemma_version", "gemma4-12b-ltx-v1")
    save(module(Gemma4UnifiedForConditionalGeneration(config)), out / "text_encoder")
    save(module(LTX2VideoTransformer3DModel(
        in_channels=4, out_channels=4, num_layers=2, num_attention_heads=2, attention_head_dim=8,
        cross_attention_dim=16, caption_channels=32, audio_in_channels=2, audio_out_channels=2,
        audio_num_attention_heads=2, audio_attention_head_dim=4, audio_cross_attention_dim=8,
        audio_scale_factor=4, rope_type="split", rope_double_precision=False, use_prompt_embeddings=False,
        cross_attn_mod=False, audio_cross_attn_mod=False, gated_attn=False, audio_gated_attn=False,
    )), out / "transformer")
    save(module(LTX2TextConnectors(
        caption_channels=32, text_proj_in_factor=3, per_modality_projections=True,
        video_hidden_dim=16, audio_hidden_dim=8, video_connector_num_layers=1,
        video_connector_num_attention_heads=2, video_connector_attention_head_dim=8,
        audio_connector_num_layers=1, audio_connector_num_attention_heads=2, audio_connector_attention_head_dim=4,
        rope_type="split", rope_double_precision=False,
    )), out / "connectors")
    save(module(AutoencoderKLLTX2Video(
        block_out_channels=(8, 16, 32, 32), decoder_block_out_channels=(8, 16, 16, 32),
        layers_per_block=(1, 1, 1, 1, 1), decoder_layers_per_block=(1, 1, 1, 1, 1),
        latent_channels=4, patch_size=4, patch_size_t=1, spatial_compression_ratio=32,
        temporal_compression_ratio=8, spatio_temporal_scaling=(True,) * 4,
        decoder_spatio_temporal_scaling=(True,) * 4, decoder_inject_noise=(False,) * 5,
        downsample_type=("spatial", "temporal", "spatiotemporal", "spatiotemporal"),
        upsample_type=("spatiotemporal", "spatiotemporal", "temporal", "spatial"),
        upsample_factor=(2, 2, 1, 2), upsample_residual=(False,) * 4,
    )), out / "vae")
    save(module(AutoencoderKLLTX2Audio(
        base_channels=2, ch_mult=(1, 1, 1), num_res_blocks=1, latent_channels=1,
        mel_bins=8, resolution=32, in_channels=2, output_channels=2,
    )), out / "audio_vae")
    save(module(LTX2VocoderWithBWE(
        hidden_channels=128, in_channels=16, out_channels=2, num_mel_channels=8,
        act_fn="leaky_relu", antialias=False, resnet_kernel_sizes=(3,), resnet_dilations=((1, 3, 5),),
        upsample_factors=(5, 2, 2, 2, 2, 2), upsample_kernel_sizes=(11, 4, 4, 4, 4, 4),
        bwe_hidden_channels=64, bwe_in_channels=16, bwe_out_channels=2,
        bwe_act_fn="leaky_relu", bwe_antialias=False, bwe_resnet_kernel_sizes=(3,),
        bwe_resnet_dilations=((1, 3, 5),), bwe_upsample_factors=(6, 5, 2, 2, 2),
        bwe_upsample_kernel_sizes=(12, 11, 4, 4, 4), filter_length=128, window_length=128, hop_length=80,
    )).float(), out / "vocoder")
    save(module(LTX2LatentUpsamplerModel(in_channels=4, mid_channels=32, num_blocks_per_stage=1,
                                       dims=3, spatial_upsample=True, temporal_upsample=False,
                                       use_rational_resampler=False)), out / "latent_upsampler")
    save(FlowMatchEulerDiscreteScheduler(), out / "scheduler")
    names = {"text_encoder": ["transformers", "Gemma4UnifiedForConditionalGeneration"],
             "tokenizer": ["transformers", "GemmaTokenizer"], "scheduler": ["diffusers", "FlowMatchEulerDiscreteScheduler"],
             "transformer": ["diffusers", "LTX2VideoTransformer3DModel"], "connectors": ["diffusers", "LTX2TextConnectors"],
             "vae": ["diffusers", "AutoencoderKLLTX2Video"], "audio_vae": ["diffusers", "AutoencoderKLLTX2Audio"],
             "vocoder": ["diffusers", "LTX2VocoderWithBWE"], "latent_upsampler": ["diffusers", "LTX2LatentUpsamplerModel"]}
    (out / "model_index.json").write_text(json.dumps({"_class_name": "LTX2Pipeline", "_diffusers_version": "0.43.0", **names}))
    return {REF: out}


_TEXT = "A red ball rolls across a wooden table. Quiet room tone."


def _instructions(text: str, *keys: str) -> dict[str, Json]:
    return {"text": text, "references": [{"key": key, "type": "image"} for key in keys]}


# The enhancer (Gemma 4 E2B, on by default) has no tiny fixture: these cells read the text as written.
_SMALL: dict[str, Json] = {"instructions": _instructions(_TEXT), "duration": 1, "resolution": "720p", "seed": 7,
                           "prompt_upsampling": False}
CELLS: list[Cell] = [
    {"entry": "generate", "name": "text-audio", "payload": dict(_SMALL), "check": video(1280, 704, 25, 48000)},
    {"entry": "generate", "name": "silent", "payload": {**_SMALL, "audio": False}, "check": video(1280, 704, 25)},
    # se-141: the first picture the instructions mention is the first frame, as on every canvas box.
    {"entry": "generate", "name": "first-frame",
     "payload": {**_SMALL, "instructions": _instructions(f"<opening> {_TEXT}", "opening"), "aspect_ratio": "16:9"},
     "files": {"instructions.references.0.file": _shared.picture("opening", 96, 64)}, "check": video(1280, 704, 25, 48000)},
]
