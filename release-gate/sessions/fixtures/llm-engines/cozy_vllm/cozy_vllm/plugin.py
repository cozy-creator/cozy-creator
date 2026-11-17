"""The registrations: `cozy_int8_convrot` and `--load-format cozytensors` (see the package docstring)."""

from __future__ import annotations

from typing import Any

import torch
from compressed_tensors.quantization import QuantizationStrategy
from vllm.model_executor.layers.linear import LinearBase, LinearMethodBase, UnquantizedLinearMethod
from vllm.model_executor.layers.quantization import register_quantization_config
from vllm.model_executor.layers.quantization.base_config import QuantizationConfig
from vllm.model_executor.layers.quantization.compressed_tensors.schemes.compressed_tensors_w8a8_int8 import (
    CompressedTensorsW8A8Int8,
)
from vllm.model_executor.model_loader import register_model_loader
from vllm.model_executor.model_loader.base_loader import BaseModelLoader

import cozy_vllm

_H: dict[tuple[torch.device, torch.dtype], torch.Tensor] = {}


def _rotate(x: torch.Tensor) -> torch.Tensor:
    key = (x.device, x.dtype)
    h = _H.get(key)
    if h is None:
        h = _H[key] = cozy_vllm.hadamard(x.device, x.dtype)
    shape = x.shape
    return (x.reshape(-1, shape[-1] // cozy_vllm.GROUP, cozy_vllm.GROUP) @ h).reshape(shape)


class ConvRotLinearMethod(LinearMethodBase):
    def __init__(self) -> None:
        self.scheme = CompressedTensorsW8A8Int8(
            strategy=QuantizationStrategy.CHANNEL, is_static_input_scheme=False, input_symmetric=True
        )

    def create_weights(self, layer, input_size_per_partition, output_partition_sizes, input_size, output_size,
                       params_dtype, **extra_weight_attrs):
        if input_size_per_partition % cozy_vllm.GROUP:
            raise ValueError(f"int8.convrot256 needs K % 256 == 0, got {input_size_per_partition}")
        self.scheme.create_weights(
            layer=layer,
            output_partition_sizes=output_partition_sizes,
            input_size_per_partition=input_size_per_partition,
            params_dtype=params_dtype,
            weight_loader=extra_weight_attrs.get("weight_loader"),
        )

    def process_weights_after_loading(self, layer: torch.nn.Module) -> None:
        self.scheme.process_weights_after_loading(layer)

    def apply(self, layer: torch.nn.Module, x: torch.Tensor, bias: torch.Tensor | None = None) -> torch.Tensor:
        return self.scheme.apply_weights(layer, _rotate(x), bias)


@register_quantization_config("cozy_int8_convrot")
class ConvRotConfig(QuantizationConfig):
    def __init__(self, ref: str) -> None:
        super().__init__()
        self.ref = ref
        self.modules = cozy_vllm.encoded_modules(ref)

    def get_name(self):
        return "cozy_int8_convrot"

    def get_supported_act_dtypes(self) -> list[torch.dtype]:
        return [torch.bfloat16, torch.float16]

    @classmethod
    def get_min_capability(cls) -> int:
        return 75

    @staticmethod
    def get_config_filenames() -> list[str]:
        return []

    @classmethod
    def from_config(cls, config: dict[str, Any]) -> "ConvRotConfig":
        return cls(config["ref"])

    def get_quant_method(self, layer: torch.nn.Module, prefix: str):
        if not isinstance(layer, LinearBase):
            return None
        held = cozy_vllm.quantized(prefix, self.modules)
        if held is None:
            raise ValueError(f"{prefix}: its stacked shards are not all int8.convrot256")
        return ConvRotLinearMethod() if held else UnquantizedLinearMethod()


@register_model_loader("cozytensors")
class CozyTensorsLoader(BaseModelLoader):
    """Weights from the machine's TensorFS store: `model_loader_extra_config={"ref": "org/name@sha256:…"}`."""

    def download_model(self, model_config) -> None:
        pass

    def load_weights(self, model: torch.nn.Module, model_config) -> None:
        ref = (self.load_config.model_loader_extra_config or {})["ref"]
        loaded = model.load_weights(cozy_vllm.tensors(ref))
        if loaded is not None:
            missing = {n for n, _ in model.named_parameters()} - set(loaded)
            if missing:
                raise ValueError(f"{len(missing)} parameters not in {ref}: {sorted(missing)[:8]}")
