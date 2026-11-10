"""Automatic CUDA graphs: a module called in a loop replays instead of running Python.

All of a handle's graphs share one pool and replay one at a time, and their tensors are scratch:
inputs are copied in, outputs cloned out. Native alone decides what happens to that memory. Eager
stays the answer for anything uncertain, and nothing here fails a request eager could serve.
"""

from __future__ import annotations

import contextlib
import functools

import torch
from torch.utils._python_dispatch import TorchDispatchMode
from torch.utils._pytree import tree_flatten, tree_map, tree_unflatten

from . import torch as vt

_SCALARS = (str, int, float, bool, type(None), torch.dtype, torch.device)
_HOST_READS = frozenset(("_local_scalar_dense", "equal", "nonzero", "nonzero_numpy", "masked_select", "unique_consecutive", "_unique2", "unique_dim"))
_RANDOM = frozenset(("rand", "rand_like", "randn", "randn_like", "randint", "randint_like", "randperm", "normal", "uniform", "bernoulli", "multinomial",
                     "exponential", "cauchy", "log_normal", "geometric", "random", "poisson", "binomial", "_standard_gamma", "_sample_dirichlet", "rrelu"))
_REPEATS = 3  # Consecutive same-key calls: only a loop repays a capture.


class _Effects(TorchDispatchMode):
    """Records what a replay would not repeat, without disturbing the one eager pass."""

    def __init__(self, protected):
        super().__init__()
        self.protected, self.found = protected, None

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        name = func._schema.name.removeprefix("aten::")
        if self.found is None:
            self.found = self._effect(func, name, args, kwargs)
        result = func(*args, **kwargs)
        if self.found is None and any(isinstance(v, torch.Tensor) and v.device.type != "cuda" and v.numel() for v in tree_flatten(result)[0]):
            self.found = f"host tensor from {name}"
        return result

    def _effect(self, func, name, args, kwargs):
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
                    if isinstance(leaf, torch.Tensor) and any(torch._C._is_alias_of(leaf, kept) for kept in self.protected):
                        return f"write to an input or buffer by {name}"
        return None


def _call(args, kwargs, device):
    """A call's structure and what a graph fixes; None when a graph can't take it."""
    leaves, spec = tree_flatten((args, kwargs))
    shape = []
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


def _configuration(module):
    """All a graph froze of its modules' Python state."""
    return [(id(child), [(k, v if type(v) in _SCALARS else id(v)) for k, v in child.__dict__.items()], len(child._forward_hooks), len(child._forward_pre_hooks))
            for child in module.modules()]


class _Graph:
    __slots__ = ("cuda", "inputs", "spec", "outputs", "regions", "configuration")


class _Candidate:
    """A root, or a composite directly under one: what stock calls in a loop."""

    __slots__ = ("module", "original", "graphs", "last", "streak", "state")

    def __init__(self, module):
        self.module, self.original = module, module.forward
        self.graphs, self.last, self.streak, self.state = {}, None, 0, None


class Graphs:
    def __init__(self, offload, roots):
        self.offload, self.owner = offload, offload.owner
        self.pool, self.active, self.previous = None, None, None
        self.counts = {"captures": 0, "replays": 0, "declines": {}}
        self.candidates = []
        inside = {id(unit.module) for unit in offload.units}
        for root in roots:
            self._install(root)
            for child in root._modules.values():
                if child is not None and sum(id(m) in inside for m in child.modules()) > 1:
                    self._install(child)

    def _install(self, module):
        candidate = _Candidate(module)

        @functools.wraps(candidate.original)
        def forward(*args, **kwargs):
            return self._dispatch(candidate, args, kwargs)

        module.forward = forward
        self.candidates.append(candidate)

    def forget(self):
        """Weights changed versions, so the next call keys differently. Old graphs wait for them."""
        for candidate in self.candidates:
            candidate.state = None

    def _dispatch(self, candidate, args, kwargs):
        if self.active is not None:
            return candidate.original(*args, **kwargs)
        with self.owner.lock:
            call = _call(args, kwargs, self.owner.device)
            if call is None:
                return self._eager(candidate, args, kwargs)
            if candidate.state is None:
                candidate.state = hash(tuple(id(weight._managed) for weight in candidate.module.parameters()))
            leaves, spec, shape = call
            key = (candidate.state, torch.is_grad_enabled(), torch.is_inference_mode_enabled(),
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

    def _eager(self, candidate, args, kwargs):
        self.active = candidate  # What it calls is part of it, not a loop of its own.
        self.previous = None
        try:
            return candidate.original(*args, **kwargs)
        finally:
            self.active = None

    def _decline(self, candidate, key, reason):
        candidate.graphs[key] = None
        self.counts["declines"][reason] = self.counts["declines"].get(reason, 0) + 1

    def _replay(self, candidate, key, graph, leaves, spec):
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
                        target.copy_(leaf)
                with vt.replay_scope(self.owner.device.index):
                    launched = True
                    vt.replay(graph.cuda, stream=stream)
                self.counts["replays"] += 1
                result = tree_map(lambda v: v.clone() if isinstance(v, torch.Tensor) else v, graph.outputs)
                vt.record_pool_stream(self.pool, stream)
                return result
        except Exception as error:  # Eager serves it, and a real fault surfaces there too.
            if launched:
                self._decline(candidate, key, f"{type(error).__name__}: {str(error)[:200]}")
            return None

    def _promote(self, candidate, key, leaves, spec, args, kwargs):
        """The eager pass is the request's answer. Then capture, and check one replay against it."""
        stream, device, module = self.owner.current(), self.owner.device.index, candidate.module
        effects = _Effects([v for v in leaves if type(v) is torch.Tensor] + list(module.buffers()))
        hooked = any(child._forward_hooks or child._forward_pre_hooks for child in module.modules())
        before = _configuration(module)
        name, token, row = f"offload:{id(module)}:{hash(key)}", None, None
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

    def _capture(self, candidate, leaves, spec, args, kwargs, units, stream, peak):
        """`peak` is what the eager pass held: the pool it needs, with room for packing."""
        with torch.inference_mode(False):
            inputs = [torch.empty(v.shape, dtype=v.dtype, device=v.device).copy_(v) if type(v) is torch.Tensor else None for v in leaves]
        statics = tree_unflatten([v if s is None else s for v, s in zip(leaves, inputs)], spec)
        fresh = self.pool is None
        if fresh:
            self.pool = tuple(torch.cuda.graph_pool_handle())
        graph, bound = _Graph(), []
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
                if type(value) is not torch.Tensor or any(torch._C._is_alias_of(value, base) for _, bases in bound for base in bases):
                    raise ValueError("a graph's outputs must be its own ordinary tensors")
            elif type(value) not in _SCALARS:
                raise ValueError("a graph's outputs must be tensors or scalars")
        # Nothing reads the pool between replays, so the core may take its pages back when idle.
        held = [value for value in tree_flatten(graph.outputs)[0] if isinstance(value, torch.Tensor)]
        vt.pool_scratch(self.pool, held, self.owner.device.index)
        graph.regions = regions
        graph.configuration = _configuration(candidate.module)
        return graph
