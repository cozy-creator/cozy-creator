"""Non-mutating preflight for adopting materialized CPU inference weights.

This module describes module slots, Parameter identities and storage aliases. It
does not select residency, create Sources, allocate CUDA memory or install hooks.
"""

from __future__ import annotations

import weakref
from collections.abc import Mapping
from dataclasses import dataclass

import torch
from torch import nn


class InventoryError(ValueError):
    """The input cannot be adopted under the plain immutable-weight contract."""


@dataclass(frozen=True)
class Slot:
    module: nn.Module
    name: str
    paths: tuple[str, ...]
    kind: str


@dataclass(frozen=True)
class Parameter:
    tensor: nn.Parameter
    slots: tuple[Slot, ...]
    storage_id: int
    shape: tuple[int, ...]
    stride: tuple[int, ...]
    byte_offset: int
    byte_end: int
    requires_grad: bool


@dataclass(frozen=True)
class Storage:
    # Keep the actual StorageImpl alive while its identity is used by this inventory.
    owner: torch.UntypedStorage
    parameters: tuple[Parameter, ...]
    nbytes: int


@dataclass(frozen=True)
class Inventory:
    parameters: tuple[Parameter, ...]
    storages: tuple[Storage, ...]
    buffers: tuple[Slot, ...]
    boundaries: tuple[str, ...]

    def facts(self) -> dict[str, int | tuple[str, ...]]:
        return {
            "parameters": len(self.parameters),
            "parameter_slots": sum(len(p.slots) for p in self.parameters),
            "parameter_paths": sum(
                len(slot.paths) for p in self.parameters for slot in p.slots
            ),
            "storages": len(self.storages),
            "host_storage_bytes": sum(storage.nbytes for storage in self.storages),
            "buffer_slots": len(self.buffers),
            "boundaries": self.boundaries,
        }


def _path(parent: str, child: str) -> str:
    return f"{parent}.{child}" if parent else child


def _walk(
    module: nn.Module, path: str, ancestors: frozenset[int], declared=frozenset()
):
    if id(module) in ancestors:
        raise InventoryError(f"module cycle at {path or '<root>'}")
    names = getattr(module, "_no_split_modules", None)
    if names is not None:
        if not isinstance(names, (tuple, list, set)) or any(
            not isinstance(name, str) for name in names
        ):
            raise InventoryError(
                f"{path or '<root>'}: invalid _no_split_modules declaration"
            )
        declared = declared | set(names)
    yield path, module, declared
    ancestors = ancestors | {id(module)}
    for name, child in module._modules.items():
        if child is not None:
            yield from _walk(child, _path(path, name), ancestors, declared)


def _plain_cpu(tensor: torch.Tensor, path: str) -> torch.UntypedStorage:
    if tensor.device.type != "cpu":
        raise InventoryError(
            f"{path}: materialized CPU weights are required for host-source adoption"
        )
    if tensor.layout != torch.strided or tensor.is_quantized or tensor.is_nested:
        raise InventoryError(f"{path}: this tensor layout needs an explicit adapter")
    if tensor.is_conj() or tensor.is_neg():
        raise InventoryError(
            f"{path}: unresolved conjugate/negative view bits need an adapter"
        )
    return tensor.untyped_storage()


def inspect_cpu_roots(roots: Mapping[str, nn.Module]) -> Inventory:
    """Inspect real model objects without altering slots, flags, storage or hooks.

    Eval mode is required. Normal requires_grad=True Parameters are inventoried for
    inference freezing, but existing gradients and unsupported swap ownership refuse.
    Shared modules retain every path; each physical slot and Parameter is recorded once.
    Buffer placement and Source-backed/meta models are separate adapter operations.
    """
    if not roots:
        raise InventoryError("at least one module root is required")
    components = {}
    for name, module in roots.items():
        if not isinstance(name, str) or not isinstance(module, nn.Module):
            raise InventoryError("roots must map names to torch.nn.Module objects")
        components[name] = list(_walk(module, name, frozenset()))
    nodes = [node for component in components.values() for node in component]

    slots: dict[tuple[int, str, str], tuple[nn.Module, list[str], torch.Tensor]] = {}
    for path, module, _ in nodes:
        if module.training:
            raise InventoryError(
                f"{path or '<root>'}: call eval() before immutable offload"
            )
        if getattr(module, "_hf_hook", None) is not None:
            raise InventoryError(
                f"{path or '<root>'}: an existing Accelerate/offload hook needs explicit removal"
            )
        if any(name in module.__dict__ for name in ("_apply", "to", "train")):
            raise InventoryError(
                f"{path or '<root>'}: instance conversion/training overrides need an adapter"
            )
        registry = getattr(module, "_diffusers_hook", None)
        if registry is not None and getattr(registry, "hooks", True):
            raise InventoryError(
                f"{path or '<root>'}: existing Diffusers hooks need an adapter"
            )
        for kind, values in (
            ("parameter", module._parameters),
            ("buffer", module._buffers),
        ):
            for name, tensor in values.items():
                if tensor is None:
                    continue
                key = id(module), kind, name
                qualified = _path(path, name)
                if key not in slots:
                    slots[key] = module, [], tensor
                slots[key][1].append(qualified)

    parameter_slots: dict[int, list[Slot]] = {}
    tensors: dict[int, nn.Parameter] = {}
    owners: dict[int, torch.UntypedStorage] = {}
    buffers = []
    buffer_storage: dict[int, str] = {}
    for (_, kind, name), (module, paths, tensor) in slots.items():
        path = paths[0]
        if kind == "buffer":
            if type(tensor) not in (torch.Tensor, nn.Parameter):
                raise InventoryError(f"{path}: buffer tensor subclass needs an adapter")
            storage = _plain_cpu(tensor, path)
            buffer_storage[storage._cdata] = path
            buffers.append(Slot(module, name, tuple(paths), kind))
            continue
        if type(tensor) is not nn.Parameter:
            raise InventoryError(f"{path}: Parameter subclass needs an adapter")
        storage = _plain_cpu(tensor, path)
        if tensor.grad is not None or not tensor.is_leaf:
            raise InventoryError(
                f"{path}: active gradient state is not immutable inference"
            )
        if weakref.getweakrefs(tensor):
            raise InventoryError(
                f"{path}: Parameter weak references prevent identity-preserving swap"
            )
        # A count of two can be an AccumulateGrad node, an ordinary view or a
        # DLPack export. Do not mutate/autograd-poison the model to guess which.
        if tensor._use_count() != 1:
            raise InventoryError(
                f"{path}: outstanding TensorImpl owners prevent identity-preserving swap"
            )
        identity = id(tensor)
        tensors[identity] = tensor
        parameter_slots.setdefault(identity, []).append(
            Slot(module, name, tuple(paths), kind)
        )
        owners[storage._cdata] = storage

    shared = owners.keys() & buffer_storage.keys()
    if shared:
        path = buffer_storage[next(iter(shared))]
        raise InventoryError(
            f"{path}: mutable buffer storage aliases an immutable Parameter"
        )

    parameters = []
    storage_parameters: dict[int, list[Parameter]] = {}
    for identity, tensor in tensors.items():
        storage = tensor.untyped_storage()
        shape, stride = tuple(tensor.shape), tuple(tensor.stride())
        if any(step < 0 for step in stride):
            raise InventoryError(
                f"{parameter_slots[identity][0].paths[0]}: negative strides need an adapter"
            )
        offset = tensor.storage_offset() * tensor.element_size()
        end = offset
        if tensor.numel():
            end += (
                1 + sum((size - 1) * step for size, step in zip(shape, stride))
            ) * tensor.element_size()
            if offset < 0 or end > storage.nbytes():
                raise InventoryError("Parameter view exceeds its CPU storage")
        parameter = Parameter(
            tensor,
            tuple(parameter_slots[identity]),
            storage._cdata,
            shape,
            stride,
            offset,
            end,
            tensor.requires_grad,
        )
        parameters.append(parameter)
        storage_parameters.setdefault(storage._cdata, []).append(parameter)

    boundaries = []
    for root, component in components.items():
        candidates = []
        for path, module, declared in component:
            if any(cls.__name__ in declared for cls in type(module).__mro__):
                candidates.append(path)
            if isinstance(module, (nn.ModuleList, nn.Sequential)):
                children = [
                    (name, child)
                    for name, child in module._modules.items()
                    if child is not None
                ]
                if (
                    len(children) > 1
                    and len({type(child) for _, child in children}) == 1
                ):
                    candidates.extend(_path(path, name) for name, _ in children)
        boundaries.extend(candidates or [root])
    return Inventory(
        tuple(parameters),
        tuple(
            Storage(owners[key], tuple(values), owners[key].nbytes())
            for key, values in storage_parameters.items()
        ),
        tuple(buffers),
        tuple(dict.fromkeys(boundaries)),
    )
