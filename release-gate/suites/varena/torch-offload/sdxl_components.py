"""Real reduced-width SDXL components; exact stock depth/frontier, no weights download.

CPU: python sdxl_components.py --device cpu
GPU (operator only): python sdxl_components.py --device cuda:0
This qualifies review/alias gates and eager component parity, not automatic replay.
"""

from __future__ import annotations

import argparse
import collections
import json
from pathlib import Path

import diffusers
import torch
import transformers
from diffusers import (
    AutoencoderKL,
    EulerDiscreteScheduler,
    StableDiffusionXLPipeline,
    UNet2DConditionModel,
)
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_auto import (
    _boundary_frontier,
    _cold_output_reason,
    _ForwardOrigin,
    _InvocationKey,
    _module_preflight,
    _promotion_reason,
)
from tensorfs.varena._offload_sdxl import (
    configuration,
    invocation_reason,
    vae_entry_leaves,
)
from tensorfs.varena._offload_stream import component_scope
from transformers import CLIPTextConfig, CLIPTextModel, CLIPTextModelWithProjection


def components(*, reduced=True, device="cpu"):
    root = Path(__file__).with_name("sdxl-configs")
    configs = {
        key: json.loads((root / key / "config.json").read_text())
        for key in ("text_encoder", "text_encoder_2", "unet", "vae")
    }
    if reduced:
        for key, value in configs.items():
            value.pop("torch_dtype", None)
            if key.startswith("text_encoder"):
                value["dtype"] = "float32"
    if reduced:
        for key, hidden, intermediate in (
            ("text_encoder", 8, 16),
            ("text_encoder_2", 16, 32),
        ):
            configs[key].update(
                vocab_size=64,
                hidden_size=hidden,
                intermediate_size=intermediate,
                num_attention_heads=2,
                max_position_embeddings=8,
                projection_dim=16,
                bos_token_id=0,
                eos_token_id=2,
                pad_token_id=1,
            )
        configs["unet"].update(
            block_out_channels=[8, 16, 32],
            attention_head_dim=[1, 2, 4],
            cross_attention_dim=24,
            norm_num_groups=4,
            addition_time_embed_dim=4,
            projection_class_embeddings_input_dim=40,
            sample_size=8,
        )
        configs["vae"].update(
            block_out_channels=[8, 16, 16, 16], norm_num_groups=4, sample_size=32
        )
    with torch.device(device):
        values = {
            "text_encoder": CLIPTextModel(CLIPTextConfig(**configs["text_encoder"])),
            "text_encoder_2": CLIPTextModelWithProjection(
                CLIPTextConfig(**configs["text_encoder_2"])
            ),
            "unet": UNet2DConditionModel.from_config(configs["unet"]),
            "vae": AutoencoderKL.from_config(configs["vae"]),
        }
    for model in values.values():
        model.eval()
    return values


def run(device):
    from tensorfs.varena._offload_inventory import inspect_cpu_roots

    if device.type == "cuda":
        device = torch.device("cuda", 0 if device.index is None else device.index)
        vt.install(device.index)  # Before any baseline CUDA tensor/model creation.
    torch.manual_seed(715)
    models = components()
    inventory = inspect_cpu_roots(models)
    originals = {
        id(child): child.forward
        for model in models.values()
        for child in model.modules()
    }
    pipeline = StableDiffusionXLPipeline(
        **models,
        tokenizer=None,
        tokenizer_2=None,
        scheduler=EulerDiscreteScheduler(),
        add_watermarker=False,
    )
    tokens = torch.tensor([[0, 3, 4, 5, 6, 7, 8, 2]], device=device)
    noise = torch.randn(1, 4, 8, 8, device=device)
    image = torch.randn(1, 3, 32, 32, device=device)
    cond = torch.randn(1, 8, 24, device=device)
    added = {
        "text_embeds": torch.randn(1, 16, device=device),
        "time_ids": torch.tensor([[32, 32, 0, 0, 32, 32]], device=device),
    }

    def execute():
        first = models["text_encoder"](tokens, output_hidden_states=True)
        second = models["text_encoder_2"](tokens, output_hidden_states=True)
        denoise = models["unet"](
            noise, 7, encoder_hidden_states=cond, added_cond_kwargs=added
        ).sample
        encoded = models["vae"].encode(image)
        decoded = models["vae"].decode(noise)
        return (
            first.last_hidden_state,
            second.text_embeds,
            denoise,
            encoded,
            decoded,
            first,
            second,
        )

    # Baseline uses actual same component weights before Source adoption. GPU
    # copies are explicit and moved back before CPU-source adoption.
    for model in models.values():
        model.to(device)
    with torch.inference_mode():
        baseline = execute()
        expected = tuple(
            value.detach().clone()
            for value in (
                *baseline[:3],
                baseline[3].latent_dist.mode(),
                baseline[4].sample,
            )
        )
        expected_hidden = tuple(
            tuple(value.clone() for value in output.hidden_states)
            for output in baseline[5:]
        )
    del baseline
    for model in models.values():
        model.to("cpu")
    owner = vt.offload(pipeline, device=device, graphs="eager")
    origins = {
        id(child): _ForwardOrigin(child.forward, originals[id(child)])
        for model in models.values()
        for child in model.modules()
    }
    frontier, nested = _boundary_frontier(owner)
    assert len(inventory.boundaries) == 178 and len(frontier) == 83
    calls = []
    hooks = [
        boundary.module.register_forward_hook(
            lambda child, args, kwargs, result: calls.append(
                (child, args, kwargs, result)
            ),
            with_kwargs=True,
        )
        for boundary in frontier
    ]
    try:
        with torch.inference_mode():
            actual = execute()
    finally:
        for hook in hooks:
            hook.remove()
    class_counts = collections.Counter()
    declined = collections.Counter()
    qualified = set()
    boundary_by_id = {id(boundary.module): boundary for boundary in frontier}
    for child, args, kwargs, result in calls:
        reason = _promotion_reason(
            owner, child, args, kwargs, result, forward_origins=origins
        )
        if type(child).__name__ == "CLIPEncoderLayer":
            assert reason == "custom_module_hooks_require_adapter", reason
            declined[reason] += 1
            continue
        assert reason is None, (type(child).__name__, reason, kwargs.keys())
        state = owner._automatic._states[id(child)]
        assert (
            owner._automatic._reason(owner, state, originals[id(child)], args, kwargs)
            is None
        )
        # Exercise actual cold inputs against the immutable key/tree/alias rules,
        # not only the class/effect whitelist.
        _InvocationKey.from_call(owner, boundary_by_id[id(child)], args, kwargs)
        qualified.add(id(child))
        class_counts[type(child).__name__] += 1
    assert len(qualified) == 39 and sum(declined.values()) == 44, (
        len(qualified),
        class_counts,
        declined,
    )
    with torch.inference_mode():
        for left, right in zip(
            expected, (*actual[:3], actual[3].latent_dist.mode(), actual[4].sample)
        ):
            torch.testing.assert_close(left, right, rtol=2e-5, atol=2e-6)
        assert actual[0] is actual[5].last_hidden_state
        assert actual[1] is actual[6].text_embeds
        for expected_states, output in zip(expected_hidden, actual[5:]):
            assert type(output.hidden_states) is tuple and output.attentions is None
            assert len(expected_states) == len(output.hidden_states)
            for left, right in zip(expected_states, output.hidden_states):
                torch.testing.assert_close(left, right, rtol=2e-5, atol=2e-6)
        # Real repeated eager components: extra reviewed eval execution must not
        # change CPU/CUDA RNG state. This is not a proof of arbitrary Python code.
        before = torch.get_rng_state().clone()
        gpu_before = (
            torch.cuda.get_rng_state(device).clone() if device.type == "cuda" else None
        )
        again = execute()
        assert torch.equal(before, torch.get_rng_state())
        if gpu_before is not None:
            assert torch.equal(gpu_before, torch.cuda.get_rng_state(device))
        for left, right in zip(
            expected, (*again[:3], again[3].latent_dist.mode(), again[4].sample)
        ):
            torch.testing.assert_close(left, right, rtol=2e-5, atol=2e-6)

        # Actual encode/decode objects stay identical through explicit method
        # component handoff. No sample() call is used to discover tensor leaves.
        for method, value in (("encode", image), ("decode", noise)):
            with component_scope(owner._owner, (value,)) as scope:
                result = getattr(models["vae"], method)(value)
                leaves = vae_entry_leaves(
                    models["vae"],
                    method,
                    result,
                    originals=owner._method_origins.get(id(models["vae"])),
                )
                assert leaves is not None, method
                assert scope.output(result, values=leaves) is result
                assert (
                    method != "encode"
                    or result.latent_dist.mode() is result.latent_dist.mean
                )
                witness = scope.checkpoint()
                assert witness is not None
                witness.wait()
                assert witness.checked
            if device.type == "cpu":
                assert not witness.can_train

    # Actual public configuration changes are declined before another body.
    embedding = models["text_encoder"].embeddings.token_embedding
    embedding.max_norm = 1.0
    assert (
        _module_preflight(owner, embedding, forward_origins=origins)
        == "embedding_renorm_requires_adapter"
    )
    embedding.max_norm = None
    block = next(
        b.module for b in frontier if type(b.module).__name__ == "BasicTransformerBlock"
    )
    block._chunk_size = 1
    assert (
        _module_preflight(owner, block, forward_origins=origins)
        == "transformer_effects_require_adapter"
    )
    block._chunk_size = None
    attention = next(
        child for child in block.modules() if type(child).__name__ == "Attention"
    )
    original_processor = attention.processor
    from diffusers.models.attention_processor import AttnProcessor

    attention.set_processor(AttnProcessor())
    assert (
        _module_preflight(owner, block, forward_origins=origins)
        == "attention_processor_requires_adapter"
    )
    attention.set_processor(original_processor)
    clip = models["text_encoder"].encoder.layers[0]
    initial = configuration(clip)
    clip.self_attn.config._attn_implementation = "eager"
    assert configuration(clip) != initial
    clip.self_attn.config._attn_implementation = "sdpa"
    transformer = next(
        b.module for b in frontier if type(b.module).__name__ == "Transformer2DModel"
    )
    example = next(row for row in calls if row[0] is transformer)
    kwargs = dict(example[2], return_dict=True)
    assert (
        invocation_reason(transformer, originals[id(transformer)], example[1], kwargs)
        == "transformer_output_object_requires_adapter"
    )
    assert (
        _cold_output_reason(torch.nn.Identity(), (), {}, (torch.ones(1), None))
        == "output_type_requires_alias_adapter"
    )
    component_entries = owner.component_facts()["entries"]
    assert component_entries["unet"] == 2
    assert component_entries["text_encoder"] == component_entries["text_encoder_2"] == 2
    assert component_entries["vae.encode"] == component_entries["vae.decode"] == 3
    assert "vae" not in component_entries, "encode/decode evidence counted root forward"
    owner.close()
    return {
        "passed": True,
        "device": str(device),
        "scope": "actual reduced-width full-depth SDXL eager/adopted components, review/alias/config gates and explicit VAE scopes; NOT automatic capture/replay or performance",
        "torch": torch.__version__,
        "diffusers": diffusers.__version__,
        "transformers": transformers.__version__,
        "declared": len(inventory.boundaries),
        "frontier": len(frontier),
        "eligible_observed": len(qualified),
        "actual_candidate_input_keys_checked": len(qualified),
        "actual_controller_framework_preflight_checked": len(qualified),
        "clip_hidden_state_tuples_and_none_preserved": [
            len(values) for values in expected_hidden
        ],
        "subsumed": len(nested),
        "observed_calls": dict(class_counts),
        "declined_calls": dict(declined),
        "vae_encode_decode": True,
        "automatic_component_scope_entries": component_entries,
        "cuda_initialized": torch.cuda.is_initialized(),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default="cpu")
    args = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(torch.device(args.device)), indent=2))
