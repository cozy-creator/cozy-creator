"""Immutable Torch inference over native Regions. See crates/varena/TORCH-OFFLOAD.md.

Weights are metadata placeholders. Entering a block binds real tensors, whose storage owns a native
Pin, into its module slots, so its body is stock ATen with no Python per op. Reads outside a block
take the placeholder's slow path. A thread's work all runs on one explicit stream.
"""

from __future__ import annotations

import functools
import os
import threading
import weakref
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from typing import TYPE_CHECKING, Any, Literal, Protocol, TypeAlias, cast

import torch
from torch import nn
from torch.utils._python_dispatch import return_and_correct_aliasing
from torch.utils._pytree import tree_flatten, tree_map

from . import api
from . import torch as vt
from ._offload_graphs import Graphs

if TYPE_CHECKING:
    from torch._ops import OpOverload
    from torch.cuda import Stream

    from .torch import DeviceLike

#: A module slot a bound view fills: the module's `_parameters`, the slot's name, and the weight it holds between forwards.
_Slot: TypeAlias = "tuple[dict[str, torch.Tensor | None], str, _Weight]"
#: One bound view: its base's index, shape, strides, element offset, and the slots it fills.
_View: TypeAlias = "tuple[int, tuple[int, ...], tuple[int, ...], int, list[_Slot]]"
#: One base tensor per (Region, dtype): the Region's index in the unit, the dtype, and its element count.
_Base: TypeAlias = "tuple[int, torch.dtype, int]"
GraphsMode: TypeAlias = Literal["auto", "eager"]


class Pipeline(Protocol):
    """A pipeline `offload` adopts: its `components` that are Modules."""

    @property
    def components(self) -> Mapping[str, object]: ...

_REFUSED = frozenset(("data_ptr", "untyped_storage", "storage", "_typed_storage", "as_subclass", "__dlpack__", "numpy"))


class OffloadError(RuntimeError):
    """An operation falls outside immutable inference."""



class _Owner:
    """What weights need from an adopted model: never the model itself."""

    def __init__(self, index: int) -> None:
        vt.install(index)
        self.device = torch.device("cuda", index)
        self.gpu = api.open(index)
        self.stream = vt._blocking_stream(index)
        self.lock = threading.RLock()
        self.converting = False

    def current(self) -> Stream:
        """The thread's explicit stream, or, if it has none, ours from now on."""
        stream = torch.cuda.current_stream(self.device)
        if stream.cuda_stream > 2:
            return stream
        self.stream.wait_stream(stream)
        with torch.cuda.device(self.device):
            torch.cuda.set_stream(self.stream)
        return self.stream

    def place(self, versions: Sequence[_Version]) -> None:
        """A Region fills from its sources, which stay: there is no second host copy."""
        sources: list[torch.UntypedStorage] = []
        for version in versions:
            if version.source is None:
                root, whole = version.root, (version.root.nbytes // version.root.dtype.itemsize,)
                assert root.source is not None  # a root Version is made from its storage
                values = torch.empty(0, dtype=root.dtype).set_(root.source, 0, whole)  # type: ignore[call-overload]  # torch's stub lacks set_(source, offset, size)
                version.source = values.to(version.dtype).untyped_storage()
            sources.append(version.source)
        region, offsets = self.gpu.region_parts([(source.data_ptr(), v.nbytes) for source, v in zip(sources, versions)], sources)
        for version, offset in zip(versions, offsets):
            version.region, version.offset = region, offset


class _Version:
    """One immutable typed storage. Each dtype is derived once from the original bytes."""

    __slots__ = ("owner", "dtype", "nbytes", "source", "region", "offset", "root", "derived")
    owner: _Owner
    dtype: torch.dtype
    nbytes: int
    source: torch.UntypedStorage | None
    region: api.Region | None
    offset: int
    root: _Version
    derived: dict[torch.dtype, _Version]

    def __init__(self, owner: _Owner, dtype: torch.dtype, nbytes: int, source: torch.UntypedStorage | None = None,
                 root: _Version | None = None) -> None:
        self.owner, self.dtype, self.nbytes, self.source = owner, dtype, nbytes, source
        self.region, self.offset, self.root, self.derived = None, 0, root or self, {}

    def like(self, dtype: torch.dtype) -> _Version:
        root = self.root
        if dtype == root.dtype:
            return root
        if dtype not in root.derived:
            root.derived[dtype] = _Version(self.owner, dtype, root.nbytes // root.dtype.itemsize * dtype.itemsize, root=root)
        return root.derived[dtype]

    def read(self, shape: tuple[int, ...], strides: tuple[int, ...], offset: int, stream: Stream) -> torch.Tensor:
        """A real tensor over these bytes; its storage owns a native Pin."""
        if 0 in shape:
            return torch.empty_strided(shape, strides, dtype=self.dtype, device=self.owner.device)
        if self.region is None:
            self.owner.place([self])
        region = self.region
        assert region is not None  # place() gave it one
        try:
            tensor = region.tensor(dtype=self.dtype, shape=shape, strides=strides,
                                   byte_offset=self.offset + offset * self.dtype.itemsize, stream=stream.cuda_stream)
        except api.Exhausted as error:
            raise torch.OutOfMemoryError(str(error)) from error
        return tensor


class _Weight(torch.Tensor):
    """A metadata placeholder in a module slot. An op on it reads its bytes through a Use."""

    _managed: _Version
    _shape: tuple[int, ...]
    _strides: tuple[int, ...]
    _offset: int

    @staticmethod
    def __new__(cls: type[_Weight], version: _Version, shape: Sequence[int], strides: Sequence[int], offset: int = 0) -> _Weight:
        with torch.inference_mode(False):
            return torch.Tensor._make_wrapper_subclass(cls, shape, strides=strides, storage_offset=offset, dtype=version.dtype,
                                                       device=version.owner.device, layout=torch.strided, requires_grad=False)

    def __init__(self, version: _Version, shape: Sequence[int], strides: Sequence[int], offset: int = 0) -> None:
        self._managed, self._shape, self._strides, self._offset = version, tuple(shape), tuple(strides), offset

    def _metadata(self) -> torch.Tensor:
        count = self._managed.nbytes // self._managed.dtype.itemsize
        return torch.empty(count, device="meta", dtype=self._managed.dtype).as_strided(self._shape, self._strides, self._offset)

    @classmethod
    def __torch_function__(  # type: ignore[explicit-any]  # torch hands over any function's operands and result
        cls, func: Callable[..., Any], types: Iterable[type], args: Sequence[Any] = (), kwargs: dict[str, Any] | None = None
    ) -> Any:
        kwargs = kwargs or {}
        name, descriptor = getattr(func, "__name__", ""), getattr(func, "__self__", None)
        if name in _REFUSED:
            raise OffloadError("make an explicit copy before accessing managed storage")
        if name == "__set__" and descriptor is torch.Tensor.data and not args[0]._managed.owner.converting:
            raise OffloadError("managed weights are immutable")
        if (name == "__set__" and descriptor is torch.Tensor.requires_grad and args[1]) or (
            func is torch.Tensor.requires_grad_ and (args[1] if len(args) > 1 else kwargs.get("requires_grad", True))
        ):
            raise OffloadError("offload supports inference only")
        with torch._C.DisableTorchFunctionSubclass():
            return func(*args, **kwargs)

    @classmethod
    def __torch_dispatch__(  # type: ignore[explicit-any, override]  # torch hands over any ATen op's operands and result; its stub types Tensor's as the disabled default
        cls, func: OpOverload[..., Any], types: Iterable[type], args: Sequence[Any] = (), kwargs: dict[str, Any] | None = None
    ) -> Any:
        kwargs = kwargs or {}
        if func is torch.ops.aten._has_compatible_shallow_copy_type.default:
            with torch._C._DisableTorchDispatch():  # Reads keys, never bytes.
                return func(*args, **kwargs)
        leaves = tree_flatten((args, kwargs))[0]
        weights = [v for v in leaves if isinstance(v, cls)]
        owner = weights[0]._managed.owner
        if any(isinstance(v, torch.Tensor) and not isinstance(v, cls) and type(v) not in (torch.Tensor, nn.Parameter) for v in leaves):
            raise OffloadError("operand Tensor subclasses need an explicit adapter")
        if func.namespace != "aten":
            raise OffloadError(f"custom operator needs an offload adapter: {func}")
        for i, argument in enumerate(func._schema.arguments):
            value = args[i] if i < len(args) else kwargs.get(argument.name, argument.default_value)
            if argument.alias_info is not None and argument.alias_info.is_write and any(isinstance(v, cls) for v in tree_flatten(value)[0]):
                raise OffloadError(f"managed weights are immutable: {func}/{argument.name}")
        if owner.converting and func is torch.ops.aten._to_copy.default:
            device = kwargs.get("device")
            if device is not None and (device.type != owner.device.type or device.index not in (None, owner.device.index)):
                raise OffloadError("an offloaded module's device is fixed; copy individual tensors explicitly")
            if kwargs.get("memory_format") not in (None, torch.preserve_format):
                raise OffloadError("memory-format conversion needs an explicit adapter")
            weight = args[0]
            return cls(weight._managed.like(kwargs.get("dtype") or weight.dtype), weight._shape, weight._strides, weight._offset)
        # Alias operations only manipulate descriptors of the same bytes.
        if not func._schema.is_mutable and func._schema.returns and all(r.alias_info is not None for r in func._schema.returns):
            metadata = {id(w): w._metadata() for w in weights}
            swap = lambda v: metadata[id(v)] if isinstance(v, cls) else v
            result = func(*tree_map(swap, args), **tree_map(swap, kwargs))
            def source(v: torch.Tensor) -> _Weight | None:
                return next((w for w in weights if torch._C._is_alias_of(v, metadata[id(w)])), None)  # type: ignore[attr-defined]  # torch._C._is_alias_of is missing from torch's stubs

            def alias(v: object) -> object:
                if not isinstance(v, torch.Tensor):
                    return v
                match = source(v)
                assert match is not None  # every result tensor aliases a weight (checked below)
                if v.dtype != match.dtype or v.is_conj() or v.is_neg():
                    raise OffloadError("reinterpret/conjugate/negative aliases need an explicit value copy")
                return cls(match._managed, v.shape, v.stride(), cast(int, v.storage_offset()))  # eager: never a SymInt

            # Under inference mode `to` arrives whole, and only sometimes returns an alias.
            if all(source(v) is not None for v in tree_flatten(result)[0] if isinstance(v, torch.Tensor)):
                return return_and_correct_aliasing(func, args, kwargs, tree_map(alias, result))
        with owner.lock:
            stream = owner.current()
            real = {id(w): w._managed.read(w._shape, w._strides, w._offset, stream) for w in weights}
            swap = lambda v: real[id(v)] if isinstance(v, cls) else v
            result = func(*tree_map(swap, args), **tree_map(swap, kwargs))

            # Some ATen operations (_unsafe_view) return aliases they don't declare. Never let one escape.
            def retain_alias(value: object) -> object:
                if not isinstance(value, torch.Tensor):
                    return value
                for weight in weights:
                    backing = real[id(weight)]
                    if torch._C._is_alias_of(value, backing):  # type: ignore[attr-defined]  # torch._C._is_alias_of is missing from torch's stubs
                        if value.dtype != backing.dtype or value.is_conj() or value.is_neg():
                            raise OffloadError("reinterpret/conjugate/negative aliases need an explicit value copy")
                        offset = cast(int, weight._offset + value.storage_offset() - backing.storage_offset())  # eager: never a SymInt
                        return cls(weight._managed, value.shape, value.stride(), offset)
                return value

            return tree_map(retain_alias, result)


class _Unit:
    """One module's weights, bound together for its forward: its subtree's, or only its own."""

    __slots__ = ("module", "whole", "bound", "split", "regions", "bases", "views", "weights")
    module: nn.Module
    whole: bool
    bound: bool
    split: bool
    regions: Sequence[api.Region]
    bases: Sequence[_Base]
    views: Sequence[_View] | None
    weights: frozenset[int]

    def __init__(self, module: nn.Module, whole: bool) -> None:
        self.module, self.whole, self.bound, self.split = module, whole, False, False
        self.regions, self.bases, self.views, self.weights = (), (), None, frozenset()


def _blocks(root: nn.Module) -> set[int]:
    """Declared _no_split_modules, and members of a repeated layer list."""
    found: set[int] = set()
    seen: set[int] = set()

    def walk(module: nn.Module, declared: frozenset[str]) -> None:
        if id(module) in seen:
            return
        seen.add(id(module))
        declared = declared | set(getattr(module, "_no_split_modules", None) or ())
        if any(kind.__name__ in declared for kind in type(module).__mro__):
            found.add(id(module))
        children = [child for child in module._modules.values() if child is not None]
        if isinstance(module, (nn.ModuleList, nn.Sequential)) and len(children) > 1 and len({type(child) for child in children}) == 1:
            found.update(id(child) for child in children)
        for child in children:
            walk(child, declared)

    walk(root, frozenset())
    return found


def _frontier(module: nn.Module, blocks: set[int], seen: set[int], descend: bool = False) -> Iterator[_Unit]:
    """Outermost blocks, the block-free subtrees between them, and a composite parent's own weights.
    A unit is what eviction moves at once, so a huge one comes apart: reading a 2 GB block back
    every step to make 200 MB of room is most of what a tight card moves."""
    if id(module) in seen:
        return
    seen.add(id(module))
    # A container has no forward of its own to bind around.
    runs = type(module).forward is not nn.Module.forward
    huge = any(True for _ in module.children()) and sum(p.numel() * p.dtype.itemsize for p in module.parameters()) > vt.unit_bytes()
    if runs and not descend and not huge and (id(module) in blocks or not any(id(m) in blocks for m in module.modules())):
        if any(True for _ in module.parameters()):
            yield _Unit(module, True)
        return
    if any(p is not None for p in module._parameters.values()):
        yield _Unit(module, False)
    for child in module._modules.values():
        if child is not None:
            yield from _frontier(child, blocks, seen)


def _plain(tensor: torch.Tensor) -> None:
    if tensor.device.type != "cpu" or tensor.layout != torch.strided or tensor.is_quantized or tensor.is_nested:
        raise OffloadError("offload adopts plain materialized CPU tensors")
    if tensor.is_conj() or tensor.is_neg():
        raise OffloadError("resolve conjugate/negative views first")


def _preflight(roots: Sequence[nn.Module]) -> tuple[list[nn.Module], list[nn.Parameter], list[torch.Tensor]]:
    """Refuse before changing anything, so adoption can't stop halfway on the model's state."""
    modules = list(dict.fromkeys(m for root in roots for m in root.modules()))
    parameters: dict[int, nn.Parameter] = {}
    buffers: dict[int, torch.Tensor] = {}
    storages: dict[int, nn.Parameter] = {}
    views: dict[int, torch.Tensor] = {}
    for module in modules:
        name = type(module).__name__
        if module.training:
            raise OffloadError(f"{name}: call eval() before offload")
        if getattr(module, "_hf_hook", None) is not None or getattr(getattr(module, "_diffusers_hook", None), "hooks", None):
            raise OffloadError(f"{name}: remove existing Accelerate or Diffusers hooks first")
        if "_apply" in module.__dict__:
            raise OffloadError(f"{name}: already offloaded, or has a custom _apply")
        parameters.update((id(p), p) for p in module._parameters.values() if p is not None)
        buffers.update((id(b), b) for b in module._buffers.values() if b is not None)
    for parameter in parameters.values():
        _plain(parameter)
        if type(parameter) is not nn.Parameter:
            raise OffloadError("Parameter subclasses need an explicit adapter")
        if parameter.grad is not None or not parameter.is_leaf or weakref.getweakrefs(parameter) or parameter._use_count() != 1:
            raise OffloadError("a Parameter has gradients, weak references or other owners, so it can't be swapped")
        if any(step < 0 for step in parameter.stride()):
            raise OffloadError("negative strides need an explicit adapter")
        if storages.setdefault(parameter.untyped_storage()._cdata, parameter).dtype != parameter.dtype:
            raise OffloadError("mixed-dtype views of one storage need an explicit adapter")
    for buffer in buffers.values():
        _plain(buffer)
        key = buffer.untyped_storage()._cdata
        if key in storages or views.setdefault(key, buffer) is not buffer:
            raise OffloadError("a buffer sharing storage with a weight or another buffer needs an explicit adapter")
    return modules, list(parameters.values()), list(buffers.values())


class Offload:
    """An adopted model. Its modules keep this alive, and adoption is permanent."""

    def __init__(self, model: nn.Module | Pipeline, device: DeviceLike, graphs: GraphsMode) -> None:
        if graphs not in ("auto", "eager"):
            raise ValueError("graphs must be 'auto' or 'eager'")
        roots = [model] if isinstance(model, nn.Module) else [m for m in getattr(model, "components", {}).values() if isinstance(m, nn.Module)]
        if not roots:
            raise TypeError("offload requires a Module or a pipeline exposing components")
        modules, parameters, buffers = _preflight(roots)
        self.owner = owner = _Owner(vt._activation_device(device))
        owner.current()
        copies = {id(buffer): buffer.to(owner.device) for buffer in buffers}
        for module in modules:
            module._buffers.update((name, copies[id(b)]) for name, b in module._buffers.items() if b is not None)
        versions: dict[int, _Version] = {}
        for parameter in parameters:
            storage = parameter.untyped_storage()
            version = versions.setdefault(storage._cdata, _Version(owner, parameter.dtype, storage.nbytes(), storage))
            weight = nn.Parameter(_Weight(version, parameter.shape, parameter.stride(), cast(int, parameter.storage_offset())), requires_grad=False)  # eager: never a SymInt
            weight.__dict__.update(parameter.__dict__)
            torch.utils.swap_tensors(parameter, weight)
        self.units: list[_Unit] = []
        self.trace: list[_Unit] | None = None
        seen: set[int] = set()
        for root in roots:
            for unit in _frontier(root, _blocks(root), seen):
                self._install(unit)
        self.graphs = Graphs(self, roots) if graphs == "auto" else None
        for module in modules:
            module.__dict__["_apply"] = functools.partial(self._apply, module)

    def _install(self, unit: _Unit) -> None:
        original = unit.module.forward

        @functools.wraps(original)
        def forward(*args: object, **kwargs: object) -> object:  # type: ignore[misc]  # wraps a forward of any signature
            if unit.split or unit.bound:
                return original(*args, **kwargs)
            if vt._owner != os.getpid():
                vt._installed()  # Before the lock, which a fork may have inherited held.
            with self.owner.lock:
                if self.trace is not None:
                    self.trace.append(unit)
                try:
                    bases = self.bind(unit)
                except api.Exhausted as error:
                    if self._split(unit):
                        return original(*args, **kwargs)
                    raise torch.OutOfMemoryError(str(error)) from error
                unit.bound = True
                try:
                    return original(*args, **kwargs)
                finally:
                    unit.bound = False
                    self.unbind(unit, bases)

        unit.module.forward = forward
        self.units.append(unit)

    def _layout(self, unit: _Unit) -> None:
        found: dict[int, tuple[_Weight, list[_Slot]]] = {}
        for module in unit.module.modules() if unit.whole else (unit.module,):
            for name, weight in module._parameters.items():
                if weight is None:
                    continue
                if not isinstance(weight, _Weight):
                    raise OffloadError("replacing module weights after offload is unsupported")
                if 0 not in weight._shape:
                    table = cast(dict[str, torch.Tensor | None], module._parameters)  # a bind puts plain views in
                    found.setdefault(id(weight), (weight, []))[1].append((table, name, weight))
        versions = list(dict.fromkeys(weight._managed for weight, _ in found.values()))
        fresh = [version for version in versions if version.region is None]
        if fresh:
            self.owner.place(fresh)
        unit.regions = list(dict.fromkeys(cast(api.Region, version.region) for version in versions))  # placed above
        bases: dict[tuple[api.Region, torch.dtype], tuple[int, _Base]] = {}
        views: list[_View] = []
        for held, slots in found.values():
            version, size = held._managed, held._managed.dtype.itemsize
            region = cast(api.Region, version.region)  # placed above
            key = region, version.dtype
            if key not in bases:
                bases[key] = len(bases), (unit.regions.index(region), version.dtype, region.nbytes // size)
            views.append((bases[key][0], held._shape, held._strides, version.offset // size + held._offset, slots))
        unit.bases, unit.views, unit.weights = [spec for _, spec in bases.values()], views, frozenset(found)

    def bind(self, unit: _Unit) -> Sequence[torch.Tensor]:
        if unit.views is None:
            self._layout(unit)
        assert unit.views is not None  # _layout set them
        if not unit.regions:
            return ()
        stream = self.owner.current().cuda_stream
        # Outside inference mode, so views share a version counter that can catch a write.
        with torch.inference_mode(False):
            with self.owner.gpu.use(unit.regions, stream) as used:
                bases = [used.tensor(index, dtype=dtype, shape=(count,), stream=stream) for index, dtype, count in unit.bases]
            for base, shape, strides, offset, slots in unit.views:
                view = bases[base].as_strided(shape, strides, offset)
                for parameters, name, _ in slots:
                    parameters[name] = view
        return bases

    def unbind(self, unit: _Unit, bases: Sequence[torch.Tensor]) -> None:
        assert unit.views is not None  # bound units have views
        for *_, slots in unit.views:
            for parameters, name, weight in slots:
                parameters[name] = weight
        if any(base._version for base in bases):
            raise OffloadError(f"{type(unit.module).__name__}: weights were written in place")

    def _split(self, unit: _Unit) -> bool:
        """Native refused this much pinned at once, so its children bind instead."""
        versions = {cast(_Weight, weight)._managed for weight in unit.module.parameters()}  # every adopted weight is one
        children = [child for child in unit.module._modules.values() if child is not None and any(True for _ in child.parameters())]
        if not unit.whole or not children:
            return False
        for version in versions:
            version.region = None
        unit.split, unit.regions, unit.views = True, (), ()
        seen = {id(other.module) for other in self.units} - {id(unit.module)}
        for child in _frontier(unit.module, _blocks(unit.module), seen, descend=True):
            self._install(child)
        return True

    def _apply(self, module: nn.Module, fn: Callable[[torch.Tensor], torch.Tensor], recurse: bool = True) -> nn.Module:
        """Module.to, half, float and cuda all arrive here: a weight's conversion is a new version."""
        owner = self.owner
        with owner.lock:
            if any(unit.bound for unit in self.units):
                raise OffloadError("convert outside an active forward")
            owner.current()
            changes: dict[int, tuple[_Weight, torch.Tensor]] = {}
            copies: dict[int, torch.Tensor] = {}
            changed: set[int] = set()
            owner.converting = True
            try:
                with torch.inference_mode(False), torch.no_grad():  # So `to` reaches _to_copy.
                    for child in module.modules() if recurse else (module,):
                        for weight in child._parameters.values():
                            if weight is not None and id(weight) not in changes:
                                changes[id(weight)] = cast(_Weight, weight), fn(weight)  # every adopted weight is one
                        for name, buffer in child._buffers.items():
                            if buffer is not None:
                                child._buffers[name] = copies.setdefault(id(buffer), fn(buffer))
                if any(not isinstance(new, _Weight) for _, new in changes.values()):
                    raise OffloadError("arbitrary Module._apply is unsupported; use to, half, float, double or bfloat16")
                for key, (adopted, new) in changes.items():
                    assert isinstance(new, _Weight)  # checked above
                    if new._managed is not adopted._managed:
                        adopted.data = new
                        adopted._managed = new._managed
                        changed.add(key)
            finally:
                owner.converting = False
            for unit in self.units:
                if unit.weights & changed:
                    unit.views = None
            if changed and self.graphs is not None:
                self.graphs.forget()
        return module


def offload(model: nn.Module | Pipeline, device: DeviceLike = 0, *, graphs: GraphsMode = "auto") -> Offload:
    return Offload(model, device, graphs)
