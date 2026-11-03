"""Reviewed immutable eval paths, not a general Diffusers purity inference.

Only the exact implementations listed below are admitted. Library upgrades,
custom processors, hooks, checkpointing, RNG and opaque outputs need review.
"""

from __future__ import annotations

import inspect
import sys

import torch

from ._offload_review import callable_fingerprint

# Filled from the actually inspected 0.41.0 / 5.22.1 Python 3.12 implementations.
# These are path-independent code/default/closure digests, not package-version
# guesses or __wrapped__ annotations. Different Python bytecode declines safely.
_IMPLEMENTATIONS = {
    "transformers.models.clip.modeling_clip.CLIPTextEmbeddings.forward": "e7d9f4ccf80d94ebd0337ca9f03e553f1ae274cd660fbacf856663521e07af24",
    "transformers.models.clip.modeling_clip.CLIPEncoderLayer.forward": "28c5f1f24769d8a0e947f507906c56ad59b4d6d9193101316ed609bc4d02578b",
    "transformers.models.clip.modeling_clip.CLIPAttention.forward": "5538250a160b9186c63eaf5fa44c964e567783bc86d36c5318d003cf45b4e4bb",
    "transformers.models.clip.modeling_clip.CLIPMLP.forward": "0466c9fba8d68970306415522aa92fc203d350c270d16b23fcb0cfee5e67a71c",
    "transformers.models.clip.modeling_clip.eager_attention_forward": "342a33838a210505e92082054ddd07ff1bfda1331508c015e1c4c42738d43de5",
    "transformers.models.clip.modeling_clip.CLIPTextModel.forward": "9e8216ac36adc1b5ae265618ebf7cad8de4e25f50b1ac87183c8e6ae7d146c37",
    "transformers.models.clip.modeling_clip.CLIPTextModelWithProjection.forward": "881a5b3316e7996be7431636f018817ca2fe68ec42073a62c8f100597cb77e99",
    "transformers.activations.GELUActivation.forward": "e465fd3f9cc63a6e7aff631419ad02639e1a335094f11e3cd00c97c215f7aee2",
    "transformers.activations.QuickGELUActivation.forward": "1e8789900ec40c2c638c11715b1eb1643fe2b56249eaede165b96e36b599bc15",
    "transformers.modeling_layers.GradientCheckpointingLayer.__call__": "15f8ea1e2ebb5ff2f9c22449affae5ca348ac29e55233ddaccd8629f86c13981",
    "transformers.integrations.sdpa_attention.sdpa_attention_forward": "21e12077a594bea224cec9fc7ebe54e9fefafe7d8d748cae56679140b86fd65b",
    "diffusers.models.resnet.ResnetBlock2D.forward": "09cf9c9182076849c03f9fb16284f1bdbf00353ae0822696b7b2a73033a3eb4e",
    "diffusers.models.unets.unet_2d_condition.UNet2DConditionModel.forward": "e9e062cf51103658f35ddcaf345cd50f5fec7bf83fe7509f22fd4a61701d5877",
    "diffusers.models.unets.unet_2d_blocks.DownEncoderBlock2D.forward": "5f0ffbef3a394fef94b29fd99560cbfbb58dccbf7bb8978817b87a532a59e777",
    "diffusers.models.unets.unet_2d_blocks.UpDecoderBlock2D.forward": "5c9b3b3fd07866440f51fb21f63734f0cf9c51b81dbc3fee84e6753cd6eae1d6",
    "diffusers.models.unets.unet_2d_blocks.CrossAttnUpBlock2D.forward": "3f740f7770a0fff154bb213097d11fae756ca599ae2339dc6ef790fb7cacbe8a",
    "diffusers.models.unets.unet_2d_blocks.UpBlock2D.forward": "1f1acc9cc829b01424351b5fba72f53b25af1949dc2a29cdafef6d307ee3b0d3",
    "diffusers.models.unets.unet_2d_blocks.CrossAttnDownBlock2D.forward": "89d604b9f0eacae94c3bcaf7bb9b6de646b624d96dfe81bea5f623884f9fd0bb",
    "diffusers.models.transformers.transformer_2d.Transformer2DModel.forward": "57bbcfd230d8de4848636d32b83ce50276283262eb31e520a893b8a8e2ad49b5",
    "diffusers.models.transformers.transformer_2d.Transformer2DModel._operate_on_continuous_inputs": "a2c3f6be31e9bb0127465c91fc0b4a7946ee339cc6ced566be5c24e378954267",
    "diffusers.models.transformers.transformer_2d.Transformer2DModel._get_output_for_continuous_inputs": "d556964b4d4346760add02efad0c3b01b2941486fbfc4555bf4fe255c89d63d4",
    "diffusers.models.attention.BasicTransformerBlock.forward": "9ea21c3713b1c46fa7796b618a5de05326d5adf35ed3e06e33e4e42ec489c634",
    "diffusers.models.attention.FeedForward.forward": "b2f51c6dff6b0c274618a0697b31b12ed23945ff6ea51e1d2870951b656a748e",
    "diffusers.models.attention_processor.Attention.forward": "a4465c640bf880d298dcd4a982a63934244642e1c04c46da285fa1cfaf37a797",
    "diffusers.models.attention_processor.Attention.prepare_attention_mask": "2d54669db26e0b4599efe71a9439a0c6dd58af13b22014b8057cdf2b06421907",
    "diffusers.models.attention_processor.Attention.norm_encoder_hidden_states": "1e514a7acfd15dd1f2f866422b5471586572fecc8999697b45055f4f6c9527f7",
    "diffusers.models.attention_processor.AttnProcessor2_0.__call__": "cf4b368e3105df4f32ada49f580bef7243afe63e60bc3a4111fd4c140274cb22",
    "diffusers.models.activations.GEGLU.forward": "8fc4bb167442e1d2457f293c6ac6fcf818cddbdcf5cb2c81f0b63114227188f3",
    "diffusers.models.activations.GEGLU.gelu": "71e95fe3ac6070d703298f127310e9d220a5d350c48498c3a53c0f17f60b3723",
    "diffusers.models.downsampling.Downsample2D.forward": "433d1f6bcea91dc5c97a2b727d000496f8b3806bc8f9b01abe733755510d56bd",
    "diffusers.models.upsampling.Upsample2D.forward": "74083a26c1d625e3e54ecfe14dbb5fc82f3a324475df68593ea9cb8cb98ed762",
    "diffusers.models.autoencoders.autoencoder_kl.AutoencoderKL.encode": "a6121f1be7c6d741694d9688c0a5763f21d1c9fe3306dbc3af7fb438099cd638",
    "diffusers.models.autoencoders.autoencoder_kl.AutoencoderKL.decode": "ccb233892cc0a3024674acb3c52acab00bb45836ba71e1ad29cd570ed1b3a304",
    "diffusers.models.autoencoders.autoencoder_kl.AutoencoderKL._encode": "438e0e7cb90835e33fa8de7f8c952e39f79f55e1e4128b1cd2756799f45336a4",
    "diffusers.models.autoencoders.autoencoder_kl.AutoencoderKL._decode": "d7b8890ba8175e8c6dc2c32bc9f23fd301275dd20a63fd6e61ee03f988339ba6",
    "diffusers.models.autoencoders.vae.DiagonalGaussianDistribution.__init__": "f26341d58a715ec539d119332a9b22f80f53e95ed4ada3a9f3e95431f84458d3",
    "diffusers.models.autoencoders.vae.DiagonalGaussianDistribution.mode": "67263c0cd1d0ecffd5da1853900120fc9d78bc4038451e5548a7465c5fed07c7",
    "diffusers.models.autoencoders.vae.Encoder.forward": "0a2dd7d0a0f6c0578b6d575837f3055a20b9c9c7a44c6b31faa60d2f7278cafe",
    "diffusers.models.autoencoders.vae.Decoder.forward": "5a346b089cf08906776c396932f5ddbb11313798627586738e8386e03efbd200",
}

_FORWARDS = {
    "transformers.models.clip.modeling_clip": (
        "CLIPTextEmbeddings",
        "CLIPEncoderLayer",
        "CLIPAttention",
        "CLIPMLP",
    ),
    "transformers.activations": ("GELUActivation", "QuickGELUActivation"),
    "diffusers.models.resnet": ("ResnetBlock2D",),
    "diffusers.models.unets.unet_2d_blocks": (
        "DownEncoderBlock2D",
        "UpDecoderBlock2D",
        "CrossAttnUpBlock2D",
        "UpBlock2D",
    ),
    "diffusers.models.transformers.transformer_2d": ("Transformer2DModel",),
    "diffusers.models.attention": ("BasicTransformerBlock", "FeedForward"),
    "diffusers.models.attention_processor": ("Attention",),
    "diffusers.models.activations": ("GEGLU",),
    "diffusers.models.downsampling": ("Downsample2D",),
    "diffusers.models.upsampling": ("Upsample2D",),
}


def _resolve(name):
    # Do not import optional frameworks merely to classify an unknown nn.Module.
    parts = name.split(".")
    for index in range(len(parts), 0, -1):
        value = sys.modules.get(".".join(parts[:index]))
        if value is None:
            continue
        for part in parts[index:]:
            value = getattr(value, part, None)
        return value
    return None


def _matches(name, value):
    try:
        return callable_fingerprint(value) == _IMPLEMENTATIONS[name]
    except (KeyError, TypeError, ValueError):
        return False


def reviewed_forward(module):
    kind = type(module)
    if kind.__name__ not in _FORWARDS.get(kind.__module__, ()):
        return None
    name = kind.__module__ + "." + kind.__name__
    if _resolve(name) is not kind or not _matches(name + ".forward", kind.forward):
        return None
    return kind.forward


def reviewed_call(module):
    from torch import nn

    if type(module).__call__ is nn.Module.__call__:
        return True
    name = "transformers.modeling_layers.GradientCheckpointingLayer.__call__"
    return type(module).__call__ is _resolve(name) and _matches(
        name, type(module).__call__
    )


def reviewed_component_forward(module, original):
    """Eager root stream scope only; this never grants graph eligibility."""
    kind = type(module)
    name = kind.__module__ + "." + kind.__name__
    if name not in (
        "transformers.models.clip.modeling_clip.CLIPTextModel",
        "transformers.models.clip.modeling_clip.CLIPTextModelWithProjection",
        "diffusers.models.unets.unet_2d_condition.UNet2DConditionModel",
    ):
        return False
    return _resolve(name) is kind and _matches(name + ".forward", original)


def module_state(module):
    """Return an eligibility refusal and extra immutable dispatch configuration."""
    kind = type(module)
    fields = object.__getattribute__(module, "__dict__")
    name = kind.__name__
    if getattr(module, "gradient_checkpointing", False):
        return "gradient_checkpointing_requires_adapter", ()
    if any(fields.get(key) is not None for key in ("s1", "s2", "b1", "b2")):
        return "freeu_requires_adapter", ()
    extras = []
    if name == "ResnetBlock2D" and fields.get("time_embedding_norm") not in (
        None,
        "default",
        "group",
    ):
        return "resnet_adaptive_norm_requires_adapter", ()
    if name == "Transformer2DModel":
        if not fields.get("is_input_continuous"):
            return "transformer_input_mode_requires_adapter", ()
        for method in (
            "_operate_on_continuous_inputs",
            "_get_output_for_continuous_inputs",
        ):
            symbol = kind.__module__ + ".Transformer2DModel." + method
            if not _matches(symbol, getattr(module, method)):
                return "transformer_helper_requires_review", ()
    if name == "BasicTransformerBlock" and (
        fields.get("norm_type") != "layer_norm"
        or any(
            getattr(module, key, None) is not None
            for key in ("pos_embed", "fuser", "_chunk_size")
        )
    ):
        return "transformer_effects_require_adapter", ()
    if (
        kind.__module__ == "diffusers.models.attention_processor"
        and name == "Attention"
    ):
        processor = fields.get("processor")
        processor_name = kind.__module__ + ".AttnProcessor2_0"
        if (
            type(processor) is not _resolve(processor_name)
            or vars(processor)
            or not _matches(processor_name + ".__call__", type(processor).__call__)
        ):
            return "attention_processor_requires_adapter", ()
        if (
            any(
                getattr(module, key, None) is not None
                for key in ("spatial_norm", "norm_q", "norm_k")
            )
            or module.norm_cross
        ):
            return "attention_normalization_requires_adapter", ()
        for method in ("prepare_attention_mask", "norm_encoder_hidden_states"):
            if not _matches(
                kind.__module__ + ".Attention." + method, getattr(module, method)
            ):
                return "attention_helper_requires_review", ()
        extras.append(("processor", id(processor), id(type(processor).__call__)))
    if name == "CLIPAttention":
        backend = module.config._attn_implementation
        if backend not in ("eager", "sdpa"):
            return "clip_attention_backend_requires_adapter", ()
        symbol = (
            "transformers.models.clip.modeling_clip.eager_attention_forward"
            if backend == "eager"
            else "transformers.integrations.sdpa_attention.sdpa_attention_forward"
        )
        runtime = sys.modules[kind.__module__]
        selected = runtime.ALL_ATTENTION_FUNCTIONS.get_interface(
            backend, runtime.eager_attention_forward
        )
        if selected is not _resolve(symbol) or not _matches(symbol, selected):
            return "clip_attention_dispatch_requires_review", ()
        if getattr(module, "num_key_value_groups", 1) != 1:
            return "clip_attention_groups_require_adapter", ()
        extras.append(("attention_backend", backend, id(selected)))
    if name == "GELUActivation" and module.act is not torch.nn.functional.gelu:
        return "gelu_callable_requires_review", ()
    if name == "GEGLU":
        if not _matches(kind.__module__ + ".GEGLU.gelu", module.gelu):
            return "geglu_helper_requires_review", ()
        if sys.modules[kind.__module__].is_torch_npu_available():
            return "npu_requires_adapter", ()
    return None, tuple(extras)


def configuration(module):
    """Supplement ordinary scalar/tuple configuration with dynamic dispatch state."""
    from ._offload import OffloadError

    rows = []
    for child in module.modules():
        if reviewed_forward(child) is None:
            continue
        reason, state = module_state(child)
        if reason:
            raise OffloadError(reason)
        rows.append((id(child), state))
    cuda = torch.backends.cuda
    rows.append(
        (
            "sdpa",
            cuda.flash_sdp_enabled(),
            cuda.mem_efficient_sdp_enabled(),
            cuda.math_sdp_enabled(),
            cuda.cudnn_sdp_enabled(),
        )
    )
    return tuple(rows)


def invocation_reason(module, original, args, kwargs):
    """Bound the reviewed top-level path BEFORE a second authored body executes."""
    if reviewed_forward(module) is None:
        return None  # The ordinary nn/effect gate supplies the refusal.
    try:
        values = inspect.signature(original).bind(*args, **kwargs)
    except TypeError:
        return "unsupported_framework_arguments"
    values.apply_defaults()
    fields = values.arguments
    for key in ("cross_attention_kwargs", "kwargs"):
        extra = fields.get(key)
        if extra is not None and (type(extra) is not dict or extra):
            return "attention_kwargs_require_adapter"
    if fields.get("args") or fields.get("scale") is not None:
        return "deprecated_scale_requires_adapter"
    if (
        type(module).__name__ == "Transformer2DModel"
        and fields.get("return_dict") is not False
    ):
        return "transformer_output_object_requires_adapter"
    return None


def vae_entry_supported(module, originals=None):
    """Check original methods before opening a managed eager method scope."""
    base = "diffusers.models.autoencoders.autoencoder_kl.AutoencoderKL"
    if (
        type(module) is not _resolve(base)
        or getattr(module, "_hf_hook", None) is not None
    ):
        return False
    if module.use_tiling or module.use_slicing:
        return False
    originals = originals or {}
    return all(
        _matches(base + "." + key, originals.get(key, getattr(module, key)))
        for key in ("encode", "decode", "_encode", "_decode")
    )


def vae_entry_leaves(module, method, result, *, originals=None):
    """Read exact stock return owners without sampling/reconstructing a distribution.

    Used for an eager component scope, never to make encode/decode a whole graph.
    None means this opaque result needs another reviewed adapter.
    """
    if not vae_entry_supported(module, originals):
        return None
    if method == "decode":
        if type(result) is _resolve("diffusers.models.autoencoders.vae.DecoderOutput"):
            return (result.sample,)
        if (
            type(result) is tuple
            and len(result) == 1
            and type(result[0]) is torch.Tensor
        ):
            return result
        return None
    if method != "encode":
        return None
    if type(result) is _resolve(
        "diffusers.models.modeling_outputs.AutoencoderKLOutput"
    ):
        distribution = result.latent_dist
    elif type(result) is tuple and len(result) == 1:
        distribution = result[0]
    else:
        return None
    dist = "diffusers.models.autoencoders.vae.DiagonalGaussianDistribution"
    if type(distribution) is not _resolve(dist) or not _matches(
        dist + ".__init__", type(distribution).__init__
    ):
        return None
    return tuple(
        getattr(distribution, key)
        for key in ("parameters", "mean", "logvar", "std", "var")
    )
