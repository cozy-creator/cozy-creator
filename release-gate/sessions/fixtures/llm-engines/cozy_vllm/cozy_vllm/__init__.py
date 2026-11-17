"""xs-212 spike: vLLM serving Cozy's stored lanes as they are.

- `--load-format cozytensors`: weights read from the machine's TensorFS store (`tensorfs.open`), not a folder of files.
- `int8.convrot256` (quantization `cozy_int8_convrot`): the stored int8 rows of W·H and their per-row scale run as vLLM's
  int8 W8A8 (channelwise weight scale, per-token dynamic activation scale) on the activation rotated the same way:
  y = s ⊙ ((x·Ĥ) · qᵀ), Ĥ = H256/16, orthonormal and symmetric (derive/microscale.py `decode_int8_convrot256`).

Registered in every vLLM process by the `vllm.general_plugins` entry point.
"""

from __future__ import annotations

import json
import os
from collections.abc import Iterator

import torch

COMPONENT = "model"
GROUP = 256


def register() -> None:
    from cozy_vllm import plugin  # noqa: F401  registration happens on import


def checkpoint(ref: str):
    import tensorfs

    return tensorfs.open(ref, lane=os.environ.get("COZY_LANE") or None, local_files_only=True,
                         cache_dir=os.environ.get("COZY_TENSORFS_DIR") or None)


def hadamard(device: torch.device, dtype: torch.dtype) -> torch.Tensor:
    """H4⊗H4⊗H4⊗H4 / 16, the rotation `int8-convrot256/1` stores its rows in."""
    h4 = torch.tensor([[1, 1, 1, -1], [1, 1, -1, 1], [1, -1, 1, 1], [-1, 1, 1, 1]], dtype=torch.float32)
    h = h4
    for _ in range(3):
        h = torch.kron(h, h4)
    return (h / 16).to(device=device, dtype=dtype)


def tensors(ref: str) -> Iterator[tuple[str, torch.Tensor]]:
    """Every tensor of the checkpoint's model component by its Hugging Face name: plain ones as stored, an
    int8.convrot leaf as its int8 `weight` and a `[rows, 1]` f32 `weight_scale`."""
    component = checkpoint(ref).components[COMPONENT]
    names = list(component.tensors)
    for start in range(0, len(names), 64):
        batch = names[start : start + 64]
        for name, value in component.tensors.read(batch).items():
            parts = getattr(value, "parts", None)
            if parts is None:
                yield name, torch.from_dlpack(value)
                continue
            if "convrot" not in str(getattr(value, "scheme", "")):
                raise ValueError(f"{name}: encoding {value.scheme} is not int8.convrot256")
            yield name, torch.from_dlpack(parts["weight"])
            scale = torch.from_dlpack(parts["scale"] if "scale" in parts else parts["weight_scale"])
            yield name.removesuffix(".weight") + ".weight_scale", scale.reshape(-1, 1).float()


def encoded_modules(ref: str) -> list[str]:
    """The Hugging Face module names whose weights the lane encodes."""
    component = checkpoint(ref).components[COMPONENT]
    return [n.removesuffix(".weight") for n, meta in component.tensors.items() if getattr(meta, "encoding", None)]


def skeleton(ref: str, out: str) -> None:
    """A folder vLLM can configure from: the component's config (with this quantization) and its assets. No weights."""
    component = checkpoint(ref).components[COMPONENT]
    os.makedirs(out, exist_ok=True)
    config = dict(component.config)
    config["quantization_config"] = {"quant_method": "cozy_int8_convrot", "ref": ref}
    with open(os.path.join(out, "config.json"), "w") as f:
        json.dump(config, f)
    for name in component.assets:
        path = os.path.join(out, name)
        os.makedirs(os.path.dirname(path) or out, exist_ok=True)
        with open(path, "wb") as f:
            f.write(component.asset(name))


def quantized(prefix: str, modules: list[str]) -> bool | None:
    """Whether vLLM's module `prefix` holds encoded weights: True, False, or None when its stacked shards disagree."""
    stacked = {
        "qkv_proj": ("q_proj", "k_proj", "v_proj"),
        "gate_up_proj": ("gate_proj", "up_proj"),
        "in_proj_qkvz": ("in_proj_qkv", "in_proj_z"),
        "in_proj_ba": ("in_proj_b", "in_proj_a"),
    }
    if "visual" in prefix or prefix.startswith("mtp") or "layers." not in prefix:
        return False
    head, _, leaf = prefix[prefix.index("layers.") :].rpartition(".")
    found = [f"model.language_model.{head}.{s}" in modules for s in stacked.get(leaf, (leaf,))]
    if all(found):
        return True
    return False if not any(found) else None
