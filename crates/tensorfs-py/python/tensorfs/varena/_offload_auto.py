"""Metadata for automatic offload boundary selection; execution is not wired yet."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field

import torch
from torch import nn

# Initial reviewed effect boundary, not a graph-capturability promise. Unknown
# Python forwards require a separately reviewed adapter, even after a cold pass.
_REVIEWED_MODULES = (
    nn.Sequential,
    nn.ModuleList,
    nn.ModuleDict,
    nn.Linear,
    nn.Conv1d,
    nn.Conv2d,
    nn.Conv3d,
    nn.LayerNorm,
    nn.GroupNorm,
    nn.ReLU,
    nn.ReLU6,
    nn.GELU,
    nn.SiLU,
    nn.Tanh,
    nn.Sigmoid,
    nn.Identity,
    nn.Flatten,
    nn.Dropout,
    nn.Dropout1d,
    nn.Dropout2d,
    nn.Dropout3d,
)
_REVIEWED_FORWARDS = {kind: kind.forward for kind in _REVIEWED_MODULES}
_UNSUPPORTED_TREE = object()


def _function(value):
    return getattr(value, "__func__", value)


def _module_nodes(module):
    pending, seen = [module], set()
    while pending:
        child = pending.pop()
        if id(child) in seen:
            continue
        seen.add(id(child))
        yield child
        fields = object.__getattribute__(child, "__dict__")
        pending.extend(
            value for value in fields.get("_modules", {}).values() if value is not None
        )


def _registered_tensors(module, kind):
    for child in _module_nodes(module):
        fields = object.__getattribute__(child, "__dict__")
        yield from (
            value for value in fields.get(kind, {}).values() if value is not None
        )


@dataclass(frozen=True)
class _ForwardOrigin:
    installed: object = field(repr=False)
    original: object = field(repr=False)


def _module_preflight(owner, module, *, forward_origins):
    """Decline known/unknown effects before a capture body can execute again.

    Origins are recorded from actual originals/installed wrappers during adoption,
    not inferred by trusting a user function's __wrapped__ annotation afterward.
    """
    from torch.nn.modules import module as module_runtime

    from ._offload import _Weight

    if any(
        getattr(module_runtime, name)
        for name in (
            "_global_forward_hooks",
            "_global_forward_pre_hooks",
            "_global_backward_hooks",
            "_global_backward_pre_hooks",
        )
    ):
        return "global_module_hooks_require_adapter"
    owned_hooks = {handle.id for handle in owner._hooks}
    for child in _module_nodes(module):
        fields = object.__getattribute__(child, "__dict__")
        if fields.get("training", True):
            return "module_training"
        if fields.get("inplace") is True:
            return "inplace_module"
        if type(child) not in _REVIEWED_MODULES:
            return "unreviewed_module_effects"
        if any(
            value is not None
            and (
                not isinstance(value, _Weight)
                or value._version_owner.owner is not owner._owner
            )
            for value in fields.get("_parameters", {}).values()
        ):
            return "parameter_owner_changed"
        if any(
            value is not None and type(value) is not torch.Tensor
            for value in fields.get("_buffers", {}).values()
        ):
            return "buffer_tensor_type_requires_adapter"
        if (
            type(child).__call__ is not nn.Module.__call__
            or type(child)._call_impl is not nn.Module._call_impl
            or child._compiled_call_impl is not None
        ):
            return "custom_module_call_requires_adapter"
        if child._forward_hooks or child._backward_hooks or child._backward_pre_hooks:
            return "custom_module_hooks_require_adapter"
        if any(key not in owned_hooks for key in child._forward_pre_hooks):
            return "custom_module_hooks_require_adapter"
        origin = forward_origins.get(id(child))
        if origin is None:
            return "forward_origin_unrecorded"
        reviewed = _REVIEWED_FORWARDS[type(child)]
        if (
            _function(child.forward) is not _function(origin.installed)
            or _function(origin.original) is not reviewed
            or type(child).forward is not reviewed
            or not reviewed.__globals__.get("__name__", "").startswith(
                "torch.nn.modules."
            )
        ):
            return "custom_forward_requires_adapter"
    return None


def _plain_leaves(value, seen=None):
    """Walk built-in structure without invoking registered custom flatteners."""
    if seen is None:
        seen = set()
    if type(value) in (tuple, list, dict):
        if id(value) in seen:
            yield _UNSUPPORTED_TREE
            return
        seen.add(id(value))
    if type(value) in (tuple, list):
        for child in value:
            yield from _plain_leaves(child, seen)
    elif type(value) is dict:
        for key, child in value.items():
            if type(key) not in (str, int, float, bool, type(None)):
                # Surface an unsupported leaf without retaining it in a key/result.
                yield _UNSUPPORTED_TREE
            yield from _plain_leaves(child, seen)
    else:
        yield value


def _cold_output_reason(module, args, kwargs, result):
    """Metadata-only alias classification; return no Tensor/storage owners.

    This never clones/reads contents/materializes managed weights. Explicit
    GraphSegment's independent-output contract remains separate and unchanged.
    """
    from ._offload import _Weight
    from ._offload_graph import _ordinary

    outputs = tuple(_plain_leaves(result))
    input_leaves = tuple(_plain_leaves((args, kwargs)))
    if any(value is _UNSUPPORTED_TREE for value in input_leaves):
        return "input_type_requires_alias_adapter"
    inputs = tuple(value for value in input_leaves if isinstance(value, torch.Tensor))
    parameters = tuple(_registered_tensors(module, "_parameters"))
    buffers = tuple(_registered_tensors(module, "_buffers"))
    prior = []
    for value in outputs:
        if isinstance(value, _Weight):
            return "output_aliases_managed_storage"
        if not _ordinary(value):
            return "output_type_requires_alias_adapter"
        if any(value is item or torch._C._is_alias_of(value, item) for item in inputs):
            return "output_aliases_input"
        if any(
            value is item or torch._C._is_alias_of(value, item) for item in parameters
        ):
            return "output_aliases_parameter"
        if any(value is item or torch._C._is_alias_of(value, item) for item in buffers):
            return "output_aliases_buffer"
        if any(value is item or torch._C._is_alias_of(value, item) for item in prior):
            return "outputs_alias_each_other"
        prior.append(value)
    return None


def _promotion_reason(owner, module, args, kwargs, result, *, forward_origins):
    """Both independent outputs AND reviewed effects are necessary for promotion."""
    return _module_preflight(
        owner, module, forward_origins=forward_origins
    ) or _cold_output_reason(module, args, kwargs, result)


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
