"""Automatic CUDA graphs: a module called in a loop replays instead of running Python.

All of a handle's graphs share one pool and replay one at a time, and their tensors are scratch:
inputs are copied in, outputs cloned out. Native alone decides what happens to that memory. Eager
stays the answer for anything uncertain, and nothing here fails a request eager could serve.
"""

from __future__ import annotations

import contextlib
import functools
from collections.abc import Callable, Iterable, Mapping, Sequence
from typing import TYPE_CHECKING, Any, TypeAlias, TypedDict, cast

import torch
from torch import nn
from torch.utils._python_dispatch import TorchDispatchMode
from torch.utils._pytree import TreeSpec, tree_flatten, tree_map, tree_unflatten

from . import torch as vt

if TYPE_CHECKING:
    from torch._ops import OpOverload
    from torch.cuda import Stream

    from . import api
    from ._facts import PhaseObservation
    from ._offload import Offload, _Unit, _Weight
    from .torch import PoolId

#: What a graph fixes of one call leaf: a tensor's shape and dtype, or a scalar's type and value.
_Fixed: TypeAlias = "tuple[tuple[int, ...], torch.dtype] | tuple[type, object]"
#: A graph's key: the weights' versions, grad, inference and autocast modes, and the call's fixed leaves.
_Key: TypeAlias = "tuple[int, bool, bool, bool, torch.dtype, tuple[_Fixed, ...]]"
#: Each module's id, its scalar attributes (others by id), and its hook counts.
_Configuration: TypeAlias = "list[tuple[int, list[tuple[str, object]], int, int]]"

_SCALARS = (str, int, float, bool, type(None), torch.dtype, torch.device)
_HOST_READS = frozenset(("_local_scalar_dense", "equal", "nonzero", "nonzero_numpy", "masked_select", "unique_consecutive", "_unique2", "unique_dim"))
_RANDOM = frozenset(("rand", "rand_like", "randn", "randn_like", "randint", "randint_like", "randperm", "normal", "uniform", "bernoulli", "multinomial",
                     "exponential", "cauchy", "log_normal", "geometric", "random", "poisson", "binomial", "_standard_gamma", "_sample_dirichlet", "rrelu"))
_REPEATS = 3  # Consecutive same-key calls: only a loop repays a capture.


class GraphCounts(TypedDict):
    """`Graphs.counts`: captures and replays so far, and declines by reason."""

    captures: int
    replays: int
    declines: dict[str, int]


class _Effects(TorchDispatchMode):
    """Records what a replay would not repeat, without disturbing the one eager pass."""

    def __init__(self, protected: Sequence[torch.Tensor]) -> None:
        super().__init__()  # type: ignore[no-untyped-call]  # torch leaves TorchDispatchMode.__init__ unannotated
        self.protected = protected
        self.found: str | None = None

    def __torch_dispatch__(  # type: ignore[explicit-any]  # torch hands over any ATen op's operands and result
        self, func: OpOverload[..., Any], types: Iterable[type], args: Sequence[Any] = (), kwargs: dict[str, Any] | None = None
    ) -> Any:
        kwargs = kwargs or {}
        name = func._schema.name.removeprefix("aten::")
        if self.found is None:
            self.found = self._effect(func, name, args, kwargs)
        result = func(*args, **kwargs)
        if self.found is None and any(isinstance(v, torch.Tensor) and v.device.type != "cuda" and v.numel() for v in tree_flatten(result)[0]):
            self.found = f"host tensor from {name}"
        return result

    def _effect(self, func: OpOverload[..., object], name: str, args: Sequence[object], kwargs: Mapping[str, object]) -> str | None:  # type: ignore[explicit-any]  # any ATen op
        if func.namespace != "aten":
            return f"custom operator {func}"
        if name in _HOST_READS:
            return f"host read by {name}"
        if name.rstrip("_") in _RANDOM:
            return f"randomness in {name}"
        for i, argument in enumerate(func._schema.arguments):
            value = args[i] if i < len(args) else kwargs.get(argument.name, argument.default_value)
            if argument.name in ("train", "training") and value is True:
                return f"randomness in {name}"
            if isinstance(value, torch.Tensor) and value.device.type != "cuda" and value.dim():
                return f"host tensor in {name}"
            if argument.alias_info is not None and argument.alias_info.is_write:
                for leaf in tree_flatten(value)[0]:
                    if isinstance(leaf, torch.Tensor) and any(torch._C._is_alias_of(leaf, kept) for kept in self.protected):  # type: ignore[attr-defined]  # torch._C._is_alias_of is missing from torch's stubs
                        return f"write to an input or buffer by {name}"
        return None


def _call(
    args: tuple[object, ...], kwargs: Mapping[str, object], device: torch.device
) -> tuple[list[object], TreeSpec, tuple[_Fixed, ...]] | None:
    """A call's structure and what a graph fixes; None when a graph can't take it."""
    leaves, spec = tree_flatten((args, kwargs))
    shape: list[_Fixed] = []
    for leaf in leaves:
        if type(leaf) is torch.Tensor:
            if leaf.device != device or leaf.requires_grad or leaf.layout != torch.strided or leaf.is_conj() or leaf.is_neg():
                return None
            shape.append((tuple(leaf.shape), leaf.dtype))
        elif type(leaf) in _SCALARS:
            shape.append((type(leaf), leaf))
        else:
            return None
    return leaves, spec, tuple(shape)


def _configuration(module: nn.Module) -> _Configuration:
    """All a graph froze of its modules' Python state."""
    return [(id(child), [(k, v if type(v) in _SCALARS else id(v)) for k, v in child.__dict__.items()], len(child._forward_hooks), len(child._forward_pre_hooks))
            for child in module.modules()]


class _Graph:
    __slots__ = ("cuda", "inputs", "spec", "outputs", "regions", "configuration")
    cuda: torch.cuda.CUDAGraph
    inputs: list[torch.Tensor | None]
    spec: TreeSpec
    outputs: object
    regions: list[api.Region]
    configuration: _Configuration


class _Candidate:
    """A root, or a composite directly under one: what stock calls in a loop."""

    __slots__ = ("module", "original", "graphs", "last", "streak", "state")
    module: nn.Module
    original: Callable[..., object]  # type: ignore[explicit-any]  # a module's forward takes anything
    graphs: dict[_Key, _Graph | None]
    last: _Key | None
    streak: int
    state: int | None

    def __init__(self, module: nn.Module) -> None:
        self.module, self.original = module, module.forward
        self.graphs, self.last, self.streak, self.state = {}, None, 0, None


class Graphs:
    def __init__(self, offload: Offload, roots: Sequence[nn.Module]) -> None:
        self.offload, self.owner = offload, offload.owner
        self.pool: PoolId | None = None
        self.active: _Candidate | None = None
        self.previous: _Graph | None = None
        self.counts: GraphCounts = {"captures": 0, "replays": 0, "declines": {}}
        self.candidates: list[_Candidate] = []
        inside = {id(unit.module) for unit in offload.units}
        for root in roots:
            self._install(root)
            for child in root._modules.values():
                if child is not None and sum(id(m) in inside for m in child.modules()) > 1:
                    self._install(child)

    def _install(self, module: nn.Module) -> None:
        candidate = _Candidate(module)

        @functools.wraps(candidate.original)
        def forward(*args: object, **kwargs: object) -> object:  # type: ignore[misc]  # wraps a forward of any signature
            return self._dispatch(candidate, args, kwargs)

        module.forward = forward
        self.candidates.append(candidate)

    def forget(self) -> None:
        """Weights changed versions, so the next call keys differently. Old graphs wait for them."""
        for candidate in self.candidates:
            candidate.state = None

    def _dispatch(self, candidate: _Candidate, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        if self.active is not None:
            return candidate.original(*args, **kwargs)
        with self.owner.lock:
            call = _call(args, kwargs, self.owner.device)
            if call is None:
                return self._eager(candidate, args, kwargs)
            if candidate.state is None:
                candidate.state = hash(tuple(id(cast("_Weight", weight)._managed) for weight in candidate.module.parameters()))
            leaves, spec, shape = call
            key: _Key = (candidate.state, torch.is_grad_enabled(), torch.is_inference_mode_enabled(),
                   torch.is_autocast_enabled("cuda"), torch.get_autocast_dtype("cuda"), shape)
            graph = candidate.graphs.get(key)
            if graph is not None:
                result = self._replay(candidate, key, graph, leaves, spec)
                if result is not None:
                    return result
            streak = candidate.streak + 1 if candidate.last == key else 1
            candidate.last, candidate.streak = key, streak
            if key in candidate.graphs or streak < _REPEATS:
                return self._eager(candidate, args, kwargs)
            return self._promote(candidate, key, leaves, spec, args, kwargs)

    def _eager(self, candidate: _Candidate, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
        self.active = candidate  # What it calls is part of it, not a loop of its own.
        self.previous = None
        try:
            return candidate.original(*args, **kwargs)
        finally:
            self.active = None

    def _decline(self, candidate: _Candidate, key: _Key, reason: str) -> None:
        candidate.graphs[key] = None
        self.counts["declines"][reason] = self.counts["declines"].get(reason, 0) + 1

    def _replay(self, candidate: _Candidate, key: _Key, graph: _Graph, leaves: list[object], spec: TreeSpec) -> object:
        """None when the graph no longer applies; eager then serves the call."""
        if spec != graph.spec:
            return None
        if self.previous is not graph:  # Once per loop: the walk costs milliseconds.
            if _configuration(candidate.module) != graph.configuration:
                self._decline(candidate, key, "configuration changed")
                return None
            self.previous = graph
        stream = self.owner.current()
        candidate.last, candidate.streak = key, candidate.streak + 1
        launched = False
        try:
            with self.owner.gpu.use(graph.regions, stream.cuda_stream) as used:
                used.wait_ready()  # A fill still retiring would refuse the launch.
                for target, leaf in zip(graph.inputs, leaves):
                    if target is not None:
                        target.copy_(cast(torch.Tensor, leaf))  # an input slot is a tensor leaf's
                with vt.replay_scope(self.owner.device.index):
                    launched = True
                    vt.replay(graph.cuda, stream=stream)
                self.counts["replays"] += 1
                result = tree_map(lambda v: v.clone() if isinstance(v, torch.Tensor) else v, graph.outputs)
                vt.record_pool_stream(self.pool, stream)  # type: ignore[arg-type]  # bug (typing.md): a failed later capture clears the pool live graphs replay on
                return result
        except Exception as error:  # Eager serves it, and a real fault surfaces there too.
            if launched:
                self._decline(candidate, key, f"{type(error).__name__}: {str(error)[:200]}")
            return None

    def _promote(
        self, candidate: _Candidate, key: _Key, leaves: list[object], spec: TreeSpec, args: tuple[object, ...], kwargs: dict[str, object]
    ) -> object:
        """The eager pass is the request's answer. Then capture, and check one replay against it."""
        stream, device, module = self.owner.current(), self.owner.device.index, candidate.module
        effects = _Effects([v for v in leaves if type(v) is torch.Tensor] + list(module.buffers()))
        hooked = any(child._forward_hooks or child._forward_pre_hooks for child in module.modules())
        before = _configuration(module)
        name = f"offload:{id(module)}:{hash(key)}"
        token: int | None = None
        row: PhaseObservation | None = None
        self.active, self.offload.trace = candidate, []
        try:
            with contextlib.suppress(vt.PhaseOverlapError):
                token = vt.phase_enter(device, name, mode="eager")
            with effects:
                eager = candidate.original(*args, **kwargs)
            if token is not None:
                stream.synchronize()
                row, token = vt.phase_leave(device, token, completed=True), None
            units = list(dict.fromkeys(self.offload.trace))
        finally:
            if token is not None:
                vt.phase_leave(device, token, completed=False)
            self.active, self.offload.trace = None, None
        reason = effects.found or ("hooks" if hooked else None) or ("python state changed" if _configuration(module) != before else None)
        reason = reason or (None if row and row["published"] else "unmeasured")
        if reason is None:
            try:
                assert row is not None  # published above
                graph = candidate.graphs[key] = self._capture(candidate, leaves, spec, args, kwargs, units, stream, row["peak_bytes"])
                self.counts["captures"] += 1
                replayed = self._replay(candidate, key, graph, leaves, spec)
                reason = "replay refused" if replayed is None else vt.replay_differs(eager, replayed) or None
            except Exception as error:  # A graph is an optimization: any failure declines it.
                reason = f"{type(error).__name__}: {str(error)[:200]}"
        if reason is not None:
            self._decline(candidate, key, reason)
        vt.phase_forget(device, name)
        return eager

    def _capture(
        self, candidate: _Candidate, leaves: list[object], spec: TreeSpec, args: tuple[object, ...], kwargs: dict[str, object],
        units: list[_Unit], stream: Stream, peak: int,
    ) -> _Graph:
        """`peak` is what the eager pass held: the pool it needs, with room for packing."""
        with torch.inference_mode(False):
            inputs = [torch.empty(v.shape, dtype=v.dtype, device=v.device).copy_(v) if type(v) is torch.Tensor else None for v in leaves]
        statics = tree_unflatten([v if s is None else s for v, s in zip(leaves, inputs)], spec)
        fresh = self.pool is None
        if fresh:
            self.pool = cast("PoolId", tuple(torch.cuda.graph_pool_handle()))
        graph = _Graph()
        bound: list[tuple[_Unit, Sequence[torch.Tensor]]] = []
        graph.cuda, graph.inputs, graph.spec = torch.cuda.CUDAGraph(), inputs, spec
        self.active = candidate
        try:
            for unit in units:
                bound.append((unit, self.offload.bind(unit)))
                unit.bound = True
            regions = list(dict.fromkeys(region for unit in units for region in unit.regions))
            with self.owner.gpu.use(regions, stream.cuda_stream) as used:
                used.wait_ready()  # A fill still retiring would refuse the capture.
            with vt.graph(graph.cuda, pool=self.pool, stream=stream, reserve_bytes=vt.pool_bytes(peak)):
                graph.outputs = candidate.original(*statics[0], **statics[1])
        except BaseException:
            if fresh:  # Torch forgets a pool whose only capture failed; one earlier graphs hold stays.
                self.pool = None
            raise
        finally:
            self.active = None
            for unit, bases in bound:
                unit.bound = False
                self.offload.unbind(unit, bases)
        for value in tree_flatten(graph.outputs)[0]:
            if isinstance(value, torch.Tensor):
                if type(value) is not torch.Tensor or any(torch._C._is_alias_of(value, base) for _, bases in bound for base in bases):  # type: ignore[attr-defined]  # torch._C._is_alias_of is missing from torch's stubs
                    raise ValueError("a graph's outputs must be its own ordinary tensors")
            elif type(value) not in _SCALARS:
                raise ValueError("a graph's outputs must be tensors or scalars")
        # Nothing reads the pool between replays, so the core may take its pages back when idle.
        held = [value for value in tree_flatten(graph.outputs)[0] if isinstance(value, torch.Tensor)]
        vt.pool_scratch(self.pool, held, self.owner.device.index)
        graph.regions = regions
        graph.configuration = _configuration(candidate.module)
        return graph
