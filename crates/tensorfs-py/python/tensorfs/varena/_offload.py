"""Immutable eager Torch inference over native Source/Region storage."""

from __future__ import annotations

import contextvars
import functools
import threading
import types
import weakref
from contextlib import contextmanager, suppress
from dataclasses import dataclass

import torch
from torch import nn
from torch.utils._python_dispatch import return_and_correct_aliasing
from torch.utils._pytree import tree_flatten, tree_map

from ._offload_inventory import InventoryError, inspect_cpu_roots
from .sources import Source

_REBIND = contextvars.ContextVar("varena_offload_rebind", default=False)


class OffloadError(RuntimeError):
    """An operation falls outside immutable eager inference."""


class _Device:
    def __init__(self, device):
        self.device = torch.device(device)
        self.lock = threading.RLock()
        self.gpu = None
        self.stream = None
        if self.device.type == "cuda":
            from . import api
            from . import torch as vt

            index = 0 if self.device.index is None else self.device.index
            self.device = torch.device("cuda", index)
            vt.install(index)
            self.gpu = api.open(index)
            self.stream = torch.cuda.Stream(device=self.device)
        elif self.device.type != "cpu":
            raise ValueError("offload requires a CUDA ordinal/device, or explicit CPU")

    @contextmanager
    def execution(self, values):
        with self.lock:
            if torch.compiler.is_compiling() or torch.jit.is_tracing():
                raise OffloadError("offload currently supports eager inference")
            if self.stream is None:
                yield None
                return
            caller = torch.cuda.current_stream(self.device)
            stream = self.stream if caller.cuda_stream in (0, 1) else caller
            tensors = [
                v
                for v in values
                if isinstance(v, torch.Tensor)
                and not isinstance(v, _Weight)
                and v.device.type == "cuda"
            ]
            if any(v.device != self.device for v in tensors):
                raise OffloadError("cross-device operands need an explicit copy")
            # A stream record protects lifetime only. This wait orders input contents.
            if stream != caller:
                stream.wait_stream(caller)
            with torch.cuda.stream(stream):
                if torch.cuda.is_current_stream_capturing():
                    raise OffloadError(
                        "offload currently supports eager inference outside capture"
                    )
                try:
                    yield stream
                finally:
                    for tensor in tensors:
                        tensor.record_stream(stream)
                    if stream != caller:
                        caller.wait_stream(stream)


@dataclass(eq=False)
class _Version:
    source: Source
    owner: _Device
    region: object = None
    member_offset: int = 0

    def byte_offset(self, shape, strides, offset):
        # Native export bounds cover the whole packed Region. Keep alias bounds
        # local to the original physical storage before adding its pack offset.
        if (
            len(shape) != len(strides)
            or offset < 0
            or any(size < 0 for size in shape)
            or any(step < 0 for step in strides)
        ):
            raise OffloadError("invalid managed storage view")
        itemsize = getattr(torch, self.source.dtype).itemsize
        start = offset * itemsize
        extent = (
            0
            if 0 in shape
            else (1 + sum((size - 1) * step for size, step in zip(shape, strides)))
            * itemsize
        )
        if start + extent > self.source.nbytes:
            raise OffloadError("managed tensor view exceeds its original storage")
        return start

    def reservation(self):
        if self.region is None:
            self.region = self.owner.gpu.region(self.source)
        return self.region

    def tensor(self, shape, strides, offset, stream):
        dtype = getattr(torch, self.source.dtype)
        byte_offset = self.byte_offset(shape, strides, offset)
        if self.owner.gpu is None:
            # Real host execution over an immutable snapshot; no simulated allocator.
            if not self.source.data:
                return torch.empty_strided(shape, strides, dtype=dtype)
            storage = torch.frombuffer(bytearray(self.source.data), dtype=dtype)
            return storage.as_strided(shape, strides, offset)
        if 0 in shape:
            return torch.empty_strided(
                shape, strides, dtype=dtype, device=self.owner.device
            )
        return self.reservation().tensor(
            dtype=dtype,
            shape=shape,
            strides=strides,
            byte_offset=self.member_offset + byte_offset,
            stream=stream.cuda_stream,
        )


class _Weight(torch.Tensor):
    @staticmethod
    def __new__(cls, version, shape, strides, offset=0):
        version.byte_offset(shape, strides, offset)
        # Alias dispatch shares version counters. Only the immutable metadata
        # shell needs one; actual computation keeps the caller's inference mode.
        with torch.inference_mode(False):
            return torch.Tensor._make_wrapper_subclass(
                cls,
                shape,
                strides=strides,
                storage_offset=offset,
                dtype=getattr(torch, version.source.dtype),
                device=version.owner.device,
                layout=torch.strided,
                requires_grad=False,
            )

    def __init__(self, version, shape, strides, offset=0):
        self._version_owner = version
        self._shape, self._strides, self._offset = tuple(shape), tuple(strides), offset

    def _metadata(self):
        count = self._version_owner.source.nbytes // self.dtype.itemsize
        return torch.empty(count, device="meta", dtype=self.dtype).as_strided(
            self._shape, self._strides, self._offset
        )

    def data_ptr(self):
        raise OffloadError(
            "managed weights do not expose raw pointers; make an explicit copy"
        )

    def untyped_storage(self):
        raise OffloadError(
            "managed weights do not expose storage; make an explicit copy"
        )

    def storage(self):
        raise OffloadError(
            "managed weights do not expose storage; make an explicit copy"
        )

    def __dlpack__(self, *args, **kwargs):
        raise OffloadError(
            "managed weights do not export DLPack; make an explicit copy"
        )

    def numpy(self, *args, **kwargs):
        raise OffloadError("make an explicit CPU copy before exporting NumPy storage")

    @classmethod
    def __torch_function__(cls, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        name, descriptor = (
            getattr(func, "__name__", ""),
            getattr(func, "__self__", None),
        )
        if name in (
            "data_ptr",
            "untyped_storage",
            "storage",
            "_typed_storage",
            "as_subclass",
            "__dlpack__",
            "numpy",
        ):
            raise OffloadError(
                "make an explicit copy before accessing unmanaged storage"
            )
        if name == "__set__" and descriptor is torch.Tensor.data and not _REBIND.get():
            raise OffloadError("immutable managed .data assignment")
        if (
            name == "__set__"
            and descriptor is torch.Tensor.requires_grad
            and bool(args[1])
        ):
            raise OffloadError("offload supports inference only")
        if func is torch.Tensor.requires_grad_ and bool(
            args[1] if len(args) > 1 else kwargs.get("requires_grad", True)
        ):
            raise OffloadError("offload supports inference only")
        if torch.is_grad_enabled() and any(
            type(v) in (torch.Tensor, nn.Parameter) and v.requires_grad
            for v in tree_flatten((args, kwargs))[0]
        ):
            raise OffloadError(
                "gradient-bearing requests require an explicit inference context"
            )
        weights = [v for v in tree_flatten((args, kwargs))[0] if isinstance(v, cls)]
        with (
            weights[0]._version_owner.owner.lock,
            torch._C.DisableTorchFunctionSubclass(),
        ):
            return func(*args, **kwargs)

    @classmethod
    def __torch_dispatch__(cls, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        leaves = tree_flatten((args, kwargs))[0]
        weights = [v for v in leaves if isinstance(v, cls)]
        if any(
            isinstance(v, torch.Tensor)
            and not isinstance(v, cls)
            and type(v) not in (torch.Tensor, nn.Parameter)
            for v in leaves
        ):
            raise OffloadError("operand Tensor subclasses need an explicit adapter")
        if func.namespace != "aten":
            raise OffloadError(f"custom operator needs an offload adapter: {func}")
        for i, arg in enumerate(func._schema.arguments):
            value = (
                args[i] if i < len(args) else kwargs.get(arg.name, arg.default_value)
            )
            if (
                arg.alias_info is not None
                and arg.alias_info.is_write
                and any(isinstance(v, cls) for v in tree_flatten(value)[0])
            ):
                raise OffloadError(f"immutable managed argument: {func}/{arg.name}")
            if arg.name in ("training", "train") and value is True:
                raise OffloadError(f"training operator is unsupported: {func}")
        if func is torch.ops.aten._has_compatible_shallow_copy_type.default:
            # Native TensorImpl compatibility inspects existing keys, not bytes.
            # Preserve its layout/device refusals without materializing either side.
            with torch._C._DisableTorchDispatch():
                return func(*args, **kwargs)
        owner = weights[0]._version_owner.owner
        if any(w._version_owner.owner is not owner for w in weights):
            raise OffloadError(
                "operands from independent offload handles need explicit copies"
            )
        # Pure alias operations manipulate descriptors, retaining the exact version.
        if (
            not func._schema.is_mutable
            and func._schema.returns
            and all(r.alias_info is not None for r in func._schema.returns)
        ):
            metadata = {id(w): w._metadata() for w in weights}
            result = func(
                *tree_map(lambda v: metadata[id(v)] if isinstance(v, cls) else v, args),
                **tree_map(
                    lambda v: metadata[id(v)] if isinstance(v, cls) else v, kwargs
                ),
            )

            def alias(v):
                if not isinstance(v, torch.Tensor):
                    return v
                matches = [
                    w for w in weights if torch._C._is_alias_of(v, metadata[id(w)])
                ]
                if not matches:
                    raise OffloadError(f"unrecognized alias result: {func}")
                if (
                    v.dtype != getattr(torch, matches[0]._version_owner.source.dtype)
                    or v.is_conj()
                    or v.is_neg()
                ):
                    raise OffloadError(
                        "reinterpret/conjugate/negative aliases need an explicit value copy"
                    )
                return cls(
                    matches[0]._version_owner, v.shape, v.stride(), v.storage_offset()
                )

            return return_and_correct_aliasing(
                func, args, kwargs, tree_map(alias, result)
            )
        caller = (
            torch.cuda.current_stream(owner.device)
            if owner.stream is not None
            else None
        )
        with owner.execution(leaves) as stream:
            materialized = {
                id(w): w._version_owner.tensor(w._shape, w._strides, w._offset, stream)
                for w in weights
            }
            try:
                result = func(
                    *tree_map(
                        lambda v: materialized[id(v)] if isinstance(v, cls) else v, args
                    ),
                    **tree_map(
                        lambda v: materialized[id(v)] if isinstance(v, cls) else v,
                        kwargs,
                    ),
                )

                # Some ATen operations (notably _unsafe_view) return storage
                # aliases without declaring them. Never let such storage escape.
                def retain_alias(value):
                    if not isinstance(value, torch.Tensor):
                        return value
                    for weight in weights:
                        backing = materialized[id(weight)]
                        if torch._C._is_alias_of(value, backing):
                            if (
                                value.dtype != backing.dtype
                                or value.is_conj()
                                or value.is_neg()
                            ):
                                raise OffloadError(
                                    "unmanaged reinterpret alias needs an explicit copy"
                                )
                            return cls(
                                weight._version_owner,
                                value.shape,
                                value.stride(),
                                weight._offset
                                + value.storage_offset()
                                - backing.storage_offset(),
                            )
                    return value

                result = tree_map(retain_alias, result)
            finally:
                if stream is not None:
                    for value in materialized.values():
                        value.record_stream(stream)
        if caller is not None:
            for value in tree_flatten(result)[0]:
                if type(value) is torch.Tensor and value.device == owner.device:
                    value.record_stream(caller)
        return result


def _roots(model):
    if isinstance(model, nn.Module):
        return {"model": model}
    components = getattr(model, "components", None)
    if not isinstance(components, dict):
        raise TypeError("offload requires a Module or a pipeline exposing components")
    return {
        name: value
        for name, value in components.items()
        if isinstance(value, nn.Module)
    }


def _rebind(parameter, candidate):
    token = _REBIND.set(True)
    try:
        parameter.data = candidate
        parameter._version_owner = candidate._version_owner
        parameter._shape, parameter._strides, parameter._offset = (
            candidate._shape,
            candidate._strides,
            candidate._offset,
        )
    finally:
        _REBIND.reset(token)


def _unreserved_groups(definitions):
    """Group current versions only by identical existing intent memberships.

    Missing versions are unclassified: their direct-read reservation stays a
    singleton. Already reserved versions never participate in a new layout.
    """
    memberships = {}
    for name, versions in definitions.items():
        for version in versions:
            if version.region is None:
                memberships.setdefault(version, set()).add(name)
    groups = {}
    for version, names in memberships.items():
        groups.setdefault(frozenset(names), []).append(version)
    return list(groups.values())


class Offload:
    """Own an adopted model. Retain this handle while calling the model.

    Adoption is permanent; close disables module entry/conversion. Escaped managed
    aliases retain their immutable Source/device owners and remain readable.
    """

    def __init__(self, model, device=0, *, source=None):
        roots = _roots(model)
        inventory = inspect_cpu_roots(roots)
        if source is not None:
            raise TypeError(
                "materialized CPU adoption snapshots weights; external Source mappings need an explicit adapter"
            )
        snapshots = {}
        for parameter in inventory.parameters:
            if parameter.tensor.__dict__.keys() & {
                "_version_owner",
                "_shape",
                "_strides",
                "_offset",
            }:
                raise InventoryError(
                    "Parameter metadata conflicts with managed tensor fields"
                )
        buffer_storages = {}
        for slot in inventory.buffers:
            buffer = getattr(slot.module, slot.name)
            key = buffer.untyped_storage()._cdata
            if key in buffer_storages and buffer_storages[key] is not buffer:
                raise InventoryError(
                    "distinct mutable buffer storage views need an adapter"
                )
            buffer_storages[key] = buffer
        for storage in inventory.storages:
            first = storage.parameters[0].tensor
            if any(p.tensor.dtype != first.dtype for p in storage.parameters):
                raise InventoryError("mixed-dtype views of one storage need an adapter")
            snapshots[storage.owner._cdata] = Source.from_tensor(first)
        self.model, self.closed = model, False
        self._owner = _Device(
            torch.device("cuda", device) if isinstance(device, int) else device
        )
        self._roots = roots
        self._hooks = []
        self._overrides = []
        self._intent_plan = None
        self._intent_dirty = True
        self._intent_failed = False
        self._intent_frames = []
        self._intent_regions = {}
        self._boundary_modules = {}
        self._boundary_names = {}
        self._intent_entries = {}
        for name in inventory.boundaries:
            root = next(
                key for key in roots if name == key or name.startswith(key + ".")
            )
            module = roots[root].get_submodule(name[len(root) + 1 :])
            self._boundary_modules[name] = module
            self._boundary_names.setdefault(id(module), name)
            self._intent_entries[name] = 0
        versions = {
            key: _Version(value, self._owner) for key, value in snapshots.items()
        }
        candidates = []
        for p in inventory.parameters:
            candidate = nn.Parameter(
                _Weight(
                    versions[p.storage_id],
                    p.shape,
                    p.stride,
                    p.byte_offset // p.tensor.element_size(),
                ),
                requires_grad=False,
            )
            candidate.__dict__.update(p.tensor.__dict__)
            candidates.append((p.tensor, candidate))
        buffers, copied = [], {}
        for slot in inventory.buffers:
            original = getattr(slot.module, slot.name)
            if id(original) not in copied:
                copied[id(original)] = self._copy_buffer(
                    original, device=self._owner.device
                )
            buffers.append((slot, original, copied[id(original)]))
        swapped = []
        try:
            for parameter, candidate in candidates:
                torch.utils.swap_tensors(parameter, candidate)
                swapped.append((parameter, candidate))
            for slot, original, value in buffers:
                slot.module._buffers[slot.name] = value
            seen = set()
            for root in roots.values():
                for module in root.modules():
                    if id(module) not in seen:
                        seen.add(id(module))
                        self._install(module)
        except BaseException:
            for hook in self._hooks:
                hook.remove()
            for module, name, existed, original in reversed(self._overrides):
                if existed:
                    setattr(module, name, original)
                else:
                    delattr(module, name)
            for slot, original, value in buffers:
                slot.module._buffers[slot.name] = original
            for parameter, candidate in reversed(swapped):
                torch.utils.swap_tensors(parameter, candidate)
            raise
        self.facts = inventory.facts()

    def _check(self):
        if self.closed:
            raise OffloadError("offload handle is closed")
        if self._intent_failed:
            raise OffloadError(
                "native intent cleanup failed; close/retry the retained handle"
            )

    def _refresh_intent(self):
        if not self._intent_dirty:
            return
        definitions = {}
        for name, module in self._boundary_modules.items():
            versions = {}
            for parameter in module.parameters():
                if not isinstance(parameter, _Weight):
                    raise OffloadError("module parameters changed after adoption")
                version = parameter._version_owner
                if parameter.numel():
                    versions[id(version)] = version
            definitions[name] = list(versions.values())
        if self._owner.gpu is not None:
            # Stage every new layout and its Plan before publishing any version.
            # A failed constructor cannot relocate a subset of the model.
            staged, placements, plan = [], {}, None
            try:
                for group in _unreserved_groups(definitions):
                    region, offsets = self._owner.gpu.region_parts(
                        [version.source for version in group]
                    )
                    staged.append(region)
                    placements.update(
                        (version, (region, offset))
                        for version, offset in zip(group, offsets, strict=True)
                    )
                regions = {
                    name: list(
                        dict.fromkeys(
                            placements[v][0] if v in placements else v.region
                            for v in versions
                        )
                    )
                    for name, versions in definitions.items()
                }
                plan = self._owner.gpu.plan(regions)
                if self._intent_plan is not None:
                    self._intent_plan.close()
            except BaseException:
                for resource in ([plan] if plan is not None else []) + staged:
                    with suppress(Exception):
                        resource.defer_close()
                raise
            for version, (region, offset) in placements.items():
                version.region, version.member_offset = region, offset
            self._intent_plan = plan
            definitions = regions
        self._intent_regions = {
            name: len(versions) for name, versions in definitions.items()
        }
        self._intent_dirty = False

    @contextmanager
    def _intent(self, module):
        with self._owner.lock:
            self._check()
            if not self._intent_frames:
                self._refresh_intent()
            name = self._boundary_names[id(module)]
            if self._intent_plan is not None:
                self._intent_plan.activate(name)
            self._intent_frames.append(name)
            self._intent_entries[name] += 1
            try:
                yield
            finally:
                if not self._intent_failed:
                    try:
                        if self._intent_plan is not None:
                            if len(self._intent_frames) > 1:
                                self._intent_plan.activate(self._intent_frames[-2])
                            else:
                                self._intent_plan.deactivate()
                    except BaseException:
                        self._intent_failed = True
                        raise
                    else:
                        self._intent_frames.pop()

    def intent_facts(self):
        """Report declared module intent; CPU entries do not claim native activation."""
        with self._owner.lock:
            return {
                "regions": dict(self._intent_regions),
                "entries": dict(self._intent_entries),
                "active": self._intent_frames[-1] if self._intent_frames else None,
                "native": self._intent_plan is not None,
                "pending_refresh": self._intent_dirty,
                "cleanup_failed": self._intent_failed,
            }

    def _copy_buffer(self, buffer, **kwargs):
        caller = (
            torch.cuda.current_stream(self._owner.device)
            if self._owner.stream is not None
            else None
        )
        with self._owner.execution([buffer]):
            result = buffer.to(**kwargs)
        if caller is not None:
            result.record_stream(caller)
        return result

    def _install(self, module):
        reference = weakref.ref(self)

        def replace(name, function, *, bind=True):
            previous = (
                module,
                name,
                name in module.__dict__,
                module.__dict__.get(name),
            )
            setattr(
                module, name, types.MethodType(function, module) if bind else function
            )
            self._overrides.append(previous)

        def handle():
            owner = reference()
            if owner is None:
                raise OffloadError("retain the offload handle while calling this model")
            owner._check()
            return owner

        def before(current, args, kwargs):
            handle()
            if current.training:
                raise OffloadError("offload supports eval inference only")
            if torch.is_grad_enabled() and any(
                isinstance(v, torch.Tensor) and v.requires_grad
                for v in tree_flatten((args, kwargs))[0]
            ):
                raise OffloadError(
                    "gradient-bearing input requires an explicit inference context"
                )

        self._hooks.append(module.register_forward_pre_hook(before, with_kwargs=True))

        if id(module) in self._boundary_names:
            original = module.forward

            @functools.wraps(original)
            def forward(*args, **kwargs):
                before(module, args, kwargs)
                with handle()._intent(module):
                    return original(*args, **kwargs)

            replace("forward", forward, bind=False)

        def convert(current, *args, **kwargs):
            device, dtype, _nonblocking, memory_format = torch._C._nn._parse_to(
                *args, **kwargs
            )
            if memory_format is not None:
                raise OffloadError("memory-format conversion needs an explicit adapter")
            handle()._convert(current, device, dtype)
            return current

        replace("to", convert)
        for name, dtype in (
            ("half", torch.float16),
            ("float", torch.float32),
            ("double", torch.float64),
            ("bfloat16", torch.bfloat16),
        ):

            def cast(current, target=dtype):
                handle()._convert(current, None, target, floating_only=True)
                return current

            replace(name, cast)

        def apply(current, fn, recurse=True):
            handle()
            raise OffloadError(
                "arbitrary Module._apply is unsupported; use to/half/float/bfloat16/double"
            )

        replace("_apply", apply)

        def train(current, mode=True):
            handle()
            if mode:
                raise OffloadError("offload supports eval inference only")
            for child in current.modules():
                child.training = False
            return current

        replace("train", train)

    def _convert(self, module, device, dtype, *, floating_only=False):
        self._check()
        if device is not None:
            device = torch.device(device)
            if device.type != self._owner.device.type or (
                device.index is not None and device.index != self._owner.device.index
            ):
                raise OffloadError(
                    "offloaded module device is fixed; copy individual tensors explicitly"
                )
        if dtype is None:
            return
        if not (dtype.is_floating_point or dtype.is_complex):
            raise TypeError("Module.to requires a floating or complex dtype")
        with self._owner.lock:
            if self._intent_frames:
                raise OffloadError("convert outside an active declared module boundary")
            versions, changes, buffers, copied = {}, [], [], {}
            for parameter in module.parameters():
                if not isinstance(parameter, _Weight):
                    raise OffloadError("module parameters changed after adoption")
                if parameter.dtype == dtype or not (
                    parameter.is_floating_point()
                    or (not floating_only and parameter.is_complex())
                ):
                    continue
                old = parameter._version_owner
                version = versions.setdefault(id(old), None)
                if version is None:
                    version = versions[id(old)] = _Version(
                        old.source.derive(dtype), old.owner
                    )
                changes.append(
                    (
                        parameter,
                        _Weight(
                            version,
                            parameter._shape,
                            parameter._strides,
                            parameter._offset,
                        ),
                        _Weight(
                            old, parameter._shape, parameter._strides, parameter._offset
                        ),
                    )
                )
            for child in module.modules():
                for name, buffer in child._buffers.items():
                    if buffer is not None and (
                        buffer.is_floating_point()
                        or (not floating_only and buffer.is_complex())
                    ):
                        if id(buffer) not in copied:
                            copied[id(buffer)] = self._copy_buffer(buffer, dtype=dtype)
                        buffers.append((child, name, copied[id(buffer)]))
            changed = []
            try:
                for parameter, candidate, previous in changes:
                    _rebind(parameter, candidate)
                    changed.append((parameter, previous))
            except BaseException:
                for parameter, previous in reversed(changed):
                    _rebind(parameter, previous)
                raise
            for child, name, value in buffers:
                child._buffers[name] = value
            self._intent_dirty = True

    def __call__(self, *args, **kwargs):
        self._check()
        return self.model(*args, **kwargs)

    def close(self):
        with self._owner.lock:
            if self._intent_frames and not self._intent_failed:
                raise OffloadError("close outside an active declared module boundary")
            if self._intent_plan is not None:
                self._intent_plan.deactivate()
                self._intent_plan.close()
                self._intent_plan = None
            self._intent_frames.clear()
            self._intent_failed = False
            self.closed = True


def offload(model_or_pipeline, device=0, *, source=None):
    """Adopt ordinary materialized CPU eval weights for immutable eager inference.

    CUDA requires Varena installation before any CUDA initialization. Explicit
    device='cpu' runs the same Source/alias/conversion frontend on real host tensors
    for integration and inspection; it exercises no native residency or CUDA path.
    """
    return Offload(model_or_pipeline, device, source=source)
