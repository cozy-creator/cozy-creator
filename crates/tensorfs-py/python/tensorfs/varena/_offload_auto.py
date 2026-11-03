"""Metadata for automatic offload boundary selection; execution is not wired yet."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field

import torch


@dataclass(frozen=True)
class _Boundary:
    name: str
    module: object = field(repr=False)


def _boundary_frontier(owner):
    """Choose existing outermost segment boundaries, excluding composite roots."""
    roots = {id(module) for module in owner._roots.values()}
    selected, reasons, identities = [], {}, set()
    for name, module in sorted(
        owner._boundary_modules.items(), key=lambda item: (item[0].count("."), item[0])
    ):
        if id(module) in identities:
            reasons[name] = "shared_module_uses_its_existing_boundary"
            continue
        if id(module) in roots and any(module.children()):
            reasons[name] = "composite_root_requires_segment_boundaries"
            continue
        if any(name.startswith(boundary.name + ".") for boundary in selected):
            reasons[name] = "selected_ancestor_owns_this_invocation"
            continue
        selected.append(_Boundary(name, module))
        identities.add(id(module))
    return tuple(selected), reasons


def _environment(device):
    values = (
        torch.is_grad_enabled(),
        torch.is_inference_mode_enabled(),
        torch.is_autocast_enabled(device.type),
        torch.get_autocast_dtype(device.type),
        torch.get_float32_matmul_precision(),
    )
    if device.type != "cuda":
        return values
    cuda, cudnn = torch.backends.cuda, torch.backends.cudnn
    return values + (
        cudnn.enabled,
        cudnn.benchmark,
        cudnn.deterministic,
        cudnn.allow_tf32,
        cuda.matmul.allow_tf32,
        cuda.cublas_workspace_size(),
        cuda.cublaslt_workspace_size(),
        cuda.preferred_blas_library(),
    )


def _frozen_tree(spec):
    from ._offload import OffloadError

    if spec.is_leaf():
        return ("leaf",)
    if spec.type not in (tuple, list, dict):
        raise OffloadError("automatic graph input pytree type needs an adapter")
    context = ()
    if spec.type is dict:
        if any(
            type(key) not in (str, int, float, bool, type(None)) for key in spec.context
        ):
            raise OffloadError(
                "automatic graph input mapping keys must be immutable scalars"
            )
        context = tuple((type(key), key) for key in spec.context)
    return (
        spec.type,
        context,
        tuple(_frozen_tree(child) for child in spec.children()),
    )


@dataclass(frozen=True)
class _InvocationKey:
    boundary_name: str
    module_id: int
    signature: object
    parameters: tuple
    buffers: tuple
    configuration: tuple
    environment: tuple
    placements: tuple
    # Strong immutable descriptors prevent version identity reuse; never retain
    # transient input Tensors, materialized weight exports or physical Pins.
    versions: tuple = field(compare=False, repr=False)

    @classmethod
    def from_call(cls, owner, boundary, args, kwargs):
        from ._offload import OffloadError, _Weight
        from ._offload_graph import GraphSegment, _tree_signature

        module = boundary.module
        parameters = tuple(module.parameters())
        if any(
            not isinstance(value, _Weight)
            or value._version_owner.owner is not owner._owner
            for value in parameters
        ):
            raise OffloadError("automatic graph parameters changed after adoption")
        if any(child.training for child in module.modules()):
            raise OffloadError("automatic graphs require eval inference")
        signature, descriptors = _tree_signature((tuple(args), dict(kwargs)))
        versions = tuple(dict.fromkeys(value._version_owner for value in parameters))
        if any(
            version.region is not None and not version.region._handle
            for version in versions
        ):
            raise OffloadError("automatic graph Region was explicitly closed")
        return cls(
            boundary.name,
            id(module),
            (_frozen_tree(signature), tuple(descriptors)),
            GraphSegment._parameters(module),
            GraphSegment._buffers(module),
            GraphSegment._configuration(module),
            _environment(owner._owner.device),
            tuple(
                (
                    id(version),
                    version.region._handle if version.region is not None else None,
                    version.member_offset,
                    version.source.dtype,
                    version.source.nbytes,
                )
                for version in versions
            ),
            versions,
        )

    def native_key(self, owner_generation):
        # The hash contains metadata/identities only; no Tensor/user-object repr
        # or immutable Source bytes enter a native measurement key or log.
        return (
            f"offload-auto:{owner_generation}:"
            + hashlib.sha256(repr(self).encode()).hexdigest()
        )
