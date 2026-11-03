"""Measured automatic graph dispatch at reviewed immutable inference boundaries."""

from __future__ import annotations

import contextvars
import hashlib
import itertools
import os
import threading
import weakref
from contextlib import contextmanager
from dataclasses import dataclass, field

import torch
from torch import nn

# Initial immutable, RNG-free eval effect boundary, not a capture/fit promise.
# Unknown Python/random/mutable forwards require a reviewed adapter even after
# a cold pass. Eval Dropout is identity; output aliases still prevent promotion.
_REVIEWED_MODULES = (
    nn.Sequential,
    nn.ModuleList,
    nn.ModuleDict,
    nn.Linear,
    nn.Embedding,
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


class _GraphCacheEntry:
    """One native-pressure cache registration and one graph definition.

    Automatic callers retain admission for their full selection/execution. Only
    the native callback uses the graph's private lock-only retirement path.
    """

    def __init__(self, device):
        from ._runtime import CacheOwner

        self._pid = os.getpid()
        self._lock = threading.RLock()
        self._closing = False
        self._failure = None
        self.segment = None
        self.native_retirements = 0
        self._callback = self._shrink  # Retain native weak callback's actual target.
        self._native = CacheOwner(device, self._callback)
        self._native.suspend()

    def _check_process(self):
        from ._offload import OffloadError

        if self._pid != os.getpid():
            raise OffloadError("automatic cache entries cannot be used after fork")

    @contextmanager
    def admission(self):
        from ._offload import OffloadError

        self._check_process()
        with self._lock:
            if self._closing:
                raise OffloadError("automatic cache entry is closing")
            self._native.check()
            if self.segment is not None and self.segment._failure is not None:
                raise OffloadError("cached graph execution ownership is unproven")
            if self._failure is not None:
                raise OffloadError(
                    "automatic cache retirement failed"
                ) from self._failure
            yield self

    def publish(self, segment):
        from ._offload import OffloadError

        with self.admission():
            if self.segment is not None:
                raise OffloadError(
                    "retire the previous cache definition before publishing"
                )
            if segment._closed or segment._closing or segment._failure is not None:
                raise OffloadError("cannot cache a closed or unproven graph")
            self.segment = segment
            self._native.touch()

    def touch(self):
        with self.admission():
            if self.segment is not None:
                self._native.touch()

    def _shrink(self, _wanted):
        # Native intent, never a byte credit or a Python victim/size policy.
        if not self._lock.acquire(blocking=False):
            return False
        try:
            if self._closing or self._failure is not None or self.segment is None:
                return False
            try:
                if not self._retire_idle_locked():
                    return False
                self.native_retirements += 1
            except BaseException as error:
                self._failure = error
                raise
            return True  # Native ready-only trim still owns physical release.
        finally:
            self._lock.release()

    def retire_idle(self):
        """Logical key retirement; native trim still owns physical byte release."""
        with self.admission():
            return self._retire_idle_locked()

    def _retire_idle_locked(self):
        if self.segment is None:
            return True
        if not self.segment._try_retire_cached():
            return False
        self.segment = None
        self._native.suspend()
        return True

    def facts(self):
        self._check_process()
        held = self._lock.acquire(blocking=False)
        try:
            return {
                "has_definition": self.segment is not None,
                "admission_busy": not held,
                "closing": self._closing,
                "native_retirements": self.native_retirements,
                "failure": str(self._failure) if self._failure is not None else None,
                "native": self._native.facts(),
            }
        finally:
            if held:
                self._lock.release()

    def close(self):
        """Controller calls outside offload/entry locks after stopping admission."""
        self._check_process()
        with self._lock:
            self._closing = True
        self._native.close()  # Drain without the locks a callback may need.
        with self._lock:
            segment = self.segment
        if segment is not None:
            segment.close()  # Public path takes the model lock in its normal order.
        with self._lock:
            self.segment = None


@dataclass
class _Preparation:
    """One optional native pool admission, before any authored body/launch.

    Admission target is an opaque native measured peak. Cancellation changes
    preparation ownership, never claims that backing bytes were physically freed.
    """

    body_started: bool = False
    admission_attempted: bool = False
    capacity_refused: bool = False
    pool: tuple | None = None
    device: int | None = None
    failure: BaseException | None = field(default=None, repr=False)
    native_peak: int | None = None

    @property
    def can_choose_eager(self):
        return self.capacity_refused and not self.body_started and self.failure is None

    def prepare(self, device, pool, native_peak):
        from ._offload import OffloadError

        if self.body_started:
            raise OffloadError(
                "optional pool admission must precede authored execution"
            )
        if self.admission_attempted:
            raise OffloadError("automatic pool admission is one-shot")
        from . import torch as vt

        self.admission_attempted = True
        self.device, self.pool = device, tuple(pool)
        self.native_peak = native_peak
        try:
            vt.prepare_pool(self.pool, native_peak, device)
        except torch.OutOfMemoryError:
            # This catch covers ONLY the pre-body native reserve call, never
            # source/body/capture/launch errors. Cleanup refusal must propagate.
            self.cancel()
            self.capacity_refused = True
            return False
        except BaseException as error:
            # Cleanup only, never a broad RuntimeError-to-eager classification.
            self.failure = error
            self.cancel()
            raise
        return True

    def mark_body_started(self):
        from ._offload import OffloadError

        if self.failure is not None:
            raise OffloadError(
                "failed preparation cannot choose eager"
            ) from self.failure
        self.body_started = True

    def cancel(self):
        if self.pool is None:
            return
        from . import torch as vt

        try:
            vt.cancel_preparation(self.pool, self.device)
        except BaseException as error:
            self.failure = error
            raise  # Keep exact pool/device custody for fail-closed retirement.
        self.pool = None


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

    from . import _offload_sdxl as sdxl
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
        reviewed = _REVIEWED_FORWARDS.get(type(child)) or sdxl.reviewed_forward(child)
        if reviewed is None:
            return "unreviewed_module_effects"
        if type(child) is nn.Embedding and child.max_norm is not None:
            return "embedding_renorm_requires_adapter"
        reason, _ = sdxl.module_state(child)
        if reason:
            return reason
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
            not sdxl.reviewed_call(child)
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
        if (
            _function(child.forward) is not _function(origin.installed)
            or _function(origin.original) is not reviewed
            or type(child).forward is not reviewed
            or (
                type(child) in _REVIEWED_FORWARDS
                and not reviewed.__globals__.get("__name__", "").startswith(
                    "torch.nn.modules."
                )
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
    from ._offload_sdxl import invocation_reason

    origin = forward_origins.get(id(module))
    return (
        _module_preflight(owner, module, forward_origins=forward_origins)
        or invocation_reason(module, origin.original, args, kwargs)
        or _cold_output_reason(module, args, kwargs, result)
    )


@dataclass(frozen=True)
class _Boundary:
    name: str
    module: object = field(repr=False)


def _boundary_frontier(owner):
    """Choose existing outermost segment boundaries, excluding composite roots."""
    return _declared_frontier(owner._roots, owner._boundary_modules)


def _declared_frontier(roots, declarations):
    """Pure module topology selection, also usable before Source adoption."""
    roots = {id(module) for module in roots.values()}
    selected, reasons, identities = [], {}, set()
    for name, module in sorted(
        declarations.items(), key=lambda item: (item[0].count("."), item[0])
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
        torch.are_deterministic_algorithms_enabled(),
        torch.is_deterministic_algorithms_warn_only_enabled(),
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
        from ._offload_sdxl import configuration

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
            (GraphSegment._configuration(module), configuration(module)),
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


_ACTIVE_AUTOMATIC = contextvars.ContextVar("varena_automatic_segment", default=None)
_GENERATIONS = itertools.count(1)
_UNPROVEN_AUTOMATIC = []


@dataclass
class _Selection:
    boundary: _Boundary
    key: _InvocationKey | None = None
    native_key: str | None = None
    peak: int | None = None
    reason: str | None = None
    cold_decline: str | None = None
    cache: _GraphCacheEntry | None = None
    eager_calls: int = 0
    cold_observations: int = 0
    captures: int = 0
    replays: int = 0
    capacity_refusals: int = 0


class _Observation:
    """One owned native phase; an uncertain close is never blindly retried."""

    def __init__(self, device, key, mode):
        from . import torch as vt

        self.device = device
        self.token = vt.phase_enter(device, key, mode=mode)
        self.close_started = self.closed = False
        self.error = None

    def close(self, completed):
        from . import torch as vt

        self.close_started = True
        try:
            result = vt.phase_leave(self.device, self.token, completed=completed)
            self.closed = True
            return result
        except BaseException as error:
            self.error = error
            raise


class _AutomaticGraphs:
    """One key per existing frontier, using native measurements and cache requests.

    Framework support is restricted to the exact reviewed immutable SDXL paths;
    the preceding nn-only feature remains independently qualifiable.
    """

    def __init__(self, owner, mode):
        self._owner_ref = weakref.ref(owner)
        self.mode, self._pid = mode, os.getpid()
        self._generation = next(_GENERATIONS)
        frontier, self._not_selected = _boundary_frontier(owner)
        self._states = {
            id(boundary.module): _Selection(boundary) for boundary in frontier
        }
        self._closing = self._closed = False
        self._failure = None
        self._execution = None

    def check_process(self):
        from ._offload import OffloadError

        owner = self._owner_ref()
        if (
            owner is not None
            and owner._owner.gpu is not None
            and self._pid != os.getpid()
        ):
            raise OffloadError("automatic offload cannot be used after fork")

    def check(self):
        from ._offload import OffloadError

        self.check_process()
        if self._closing or self._closed:
            raise OffloadError("automatic offload admission is closed")
        if self._failure is not None:
            raise OffloadError(
                "automatic execution custody is unproven"
            ) from self._failure

    def _retain_failure(self, error, observation=None):
        self._failure = error
        _UNPROVEN_AUTOMATIC.append(
            (self, self._owner_ref(), self._execution, observation, error)
        )

    @contextmanager
    def _observe(self, key, mode):
        from .. import _ext

        owner = self._owner_ref()
        try:
            observation = _Observation(owner._owner.device.index, key, mode)
        except getattr(_ext, "PhaseOverlapError", ()):
            # Only the structured native entry refusal leaves another owned
            # token/sample untouched. Never classify a body error or its text.
            yield None
            return
        try:
            yield observation
        finally:
            if not observation.closed:
                if observation.close_started:
                    self._retain_failure(observation.error, observation)
                else:
                    try:
                        observation.close(False)
                    except BaseException as error:
                        self._retain_failure(error, observation)
                        raise

    def _reason(self, owner, state, original, args, kwargs):
        from ._offload import OffloadError
        from ._offload_graph import _tree_signature
        from ._offload_sdxl import invocation_reason

        module = state.boundary.module
        reason = _module_preflight(
            owner, module, forward_origins=owner._forward_origins
        )
        if reason:
            return reason
        reason = invocation_reason(module, original, args, kwargs)
        if reason:
            return reason
        scalar_types = (str, int, float, bool, type(None), torch.dtype, torch.device)
        if any(
            not isinstance(value, torch.Tensor) and type(value) not in scalar_types
            for value in _plain_leaves((args, kwargs))
        ):
            return "input_tree_requires_adapter"
        try:
            signature, _ = _tree_signature((tuple(args), dict(kwargs)))
            _frozen_tree(signature)
        except OffloadError:
            # This catch covers ONLY metadata input capability, not Source,
            # Region, native observation, pool admission or authored execution.
            return "input_tensor_contract_requires_adapter"
        return None

    def _select_key(self, owner, state, key):
        from . import torch as vt

        if state.key == key:
            return None
        if state.native_key is not None:
            # No sample is published by this named metadata transition. The
            # distinct name permits forgetting the old key under this lease.
            with self._observe(
                f"offload-auto:{self._generation}:key-transition", "preparation"
            ) as observation:
                if observation is None:
                    return "parent_observation_active"
                if state.cache is not None and not state.cache.retire_idle():
                    return "previous_key_in_flight"
                # False means absent; an active native key raises Busy instead.
                vt.phase_forget(owner._owner.device.index, state.native_key)
        state.key, state.native_key = key, key.native_key(self._generation)
        state.peak, state.reason, state.cold_decline = None, None, None
        return None

    def dispatch(self, module, original, args, kwargs, scope):
        from . import _offload as adapter
        from ._offload_graph import CaptureOwnershipUncertain, GraphSegment

        if (
            adapter._GRAPH_BINDING.get() is not None
            or _ACTIVE_AUTOMATIC.get() is not None
        ):
            return original(*args, **kwargs)
        state = self._states.get(id(module))
        if state is None:
            return original(*args, **kwargs)
        self.check()
        owner = self._owner_ref()
        if self.mode == "eager" or owner._owner.device.type != "cuda":
            state.reason = "eager_mode" if self.mode == "eager" else "cpu"
            state.eager_calls += 1
            return original(*args, **kwargs)
        reason = self._reason(owner, state, original, args, kwargs)
        if reason:
            state.reason = reason
            state.eager_calls += 1
            return original(*args, **kwargs)
        if scope is None:
            state.reason = "component_scope_unavailable"
            state.eager_calls += 1
            return original(*args, **kwargs)
        # Native/lifecycle errors here propagate before body, rather than being
        # confused with a Python input-capability refusal above.
        key = _InvocationKey.from_call(owner, state.boundary, args, kwargs)
        key_refusal = self._select_key(owner, state, key)
        if key_refusal is not None:
            state.reason = key_refusal
            state.eager_calls += 1
            return original(*args, **kwargs)
        if state.cold_decline is not None:
            state.reason = state.cold_decline
            state.eager_calls += 1
            return original(*args, **kwargs)
        token = _ACTIVE_AUTOMATIC.set(self)
        execution = self._execution = {
            "owner": owner,
            "state": state,
            "args": args,
            "kwargs": kwargs,
            "scope": scope,
            "result": None,
            "preparation": None,
            "graph": None,
        }
        try:
            if state.peak is None:
                with self._observe(state.native_key, "eager") as observation:
                    if observation is None:
                        state.reason = "parent_observation_active"
                        state.eager_calls += 1
                        return original(*args, **kwargs)
                    state.eager_calls += 1
                    state.cold_observations += 1
                    result = execution["result"] = original(*args, **kwargs)
                    reason = _cold_output_reason(module, args, kwargs, result)
                    witness = scope.checkpoint()
                    completed = (
                        reason is None and witness is not None and witness.can_train
                    )
                    if completed:
                        witness.wait()
                        completed = witness.checked
                    row = observation.close(completed)
                    if reason is not None:
                        state.reason = state.cold_decline = reason
                    elif row["published"]:
                        peak = row["successful_peak_bytes"]
                        if type(peak) is not int or peak < 0:
                            raise adapter.OffloadError(
                                "native automatic measurement returned no successful peak"
                            )
                        state.peak = peak  # Verbatim native capacity, no sizing policy.
                    else:
                        state.reason = "native_sample_unpublished"
                    return result  # Original cold result, exactly once.
            if state.cache is None:
                state.cache = _GraphCacheEntry(owner._owner.device.index)
            eager_reason = None
            with state.cache.admission():
                if (
                    state.cache.segment is not None
                    and state.cache.segment._closing
                    and not state.cache.retire_idle()
                ):
                    eager_reason = "retirement_pending"
                else:
                    with self._observe(None, "preparation") as observation:
                        if observation is None:
                            eager_reason = "parent_observation_active"
                        else:
                            if state.cache.segment is None:
                                preparation = execution["preparation"] = _Preparation()
                                pool = tuple(torch.cuda.graph_pool_handle())
                                if not preparation.prepare(
                                    owner._owner.device.index, pool, state.peak
                                ):
                                    state.capacity_refusals += 1
                                    eager_reason = "prebody_capacity_refused"
                                else:
                                    graph = execution["graph"] = (
                                        GraphSegment._from_prepared(
                                            owner,
                                            module,
                                            args,
                                            kwargs=kwargs,
                                            stream=scope.stream,
                                            preparation=preparation,
                                            output_inference=torch.is_inference_mode_enabled(),
                                        )
                                    )
                                    state.cache.publish(graph)
                                    state.captures += 1
                            if state.cache.segment is not None:
                                execution["graph"] = state.cache.segment
                                result = execution["result"] = state.cache.segment(
                                    *args, stream=scope.stream, **kwargs
                                )
                                state.replays += 1
                                state.reason = None
                                state.cache.touch()
                                return result
            # Release the idle cache admission AND our preparation observation
            # before choosing eager. Its allocations may now retire that cache.
            preparation = execution["preparation"]
            if preparation is not None:
                if not preparation.can_choose_eager:
                    raise adapter.OffloadError(
                        "automatic admission has no pre-body eager choice"
                    )
                preparation.mark_body_started()
            state.reason = eager_reason
            state.eager_calls += 1
            return original(*args, **kwargs)
        except BaseException as error:
            preparation = execution["preparation"]
            if preparation is not None and preparation.pool is not None:
                if preparation.body_started or preparation.failure is not None:
                    self._retain_failure(
                        error
                    )  # Constructor retained exact uncertain epoch.
                else:
                    try:
                        preparation.cancel()
                    except BaseException as cleanup:
                        self._retain_failure(cleanup)
                        raise cleanup from error
            if isinstance(error, CaptureOwnershipUncertain):
                self._retain_failure(error)
            graph = execution["graph"]
            if graph is not None and graph._failure is not None:
                self._retain_failure(error)
            raise  # No authored body/capture/launch is retried.
        finally:
            self._execution = None
            _ACTIVE_AUTOMATIC.reset(token)

    def facts(self):
        return {
            "mode": self.mode,
            "profile": "reviewed_nn_sdxl",
            "closed": self._closed,
            "closing": self._closing,
            "failure": str(self._failure) if self._failure else None,
            "not_selected": dict(self._not_selected),
            "boundaries": {
                state.boundary.name: {
                    "reason": state.reason,
                    "native_key": state.native_key,
                    "native_peak": state.peak,
                    "eager_calls": state.eager_calls,
                    "cold_observations": state.cold_observations,
                    "captures": state.captures,
                    "replays": state.replays,
                    "capacity_refusals": state.capacity_refusals,
                    "cache": state.cache.facts() if state.cache is not None else None,
                }
                for state in self._states.values()
            },
        }

    def stop(self):
        self._closing = True

    def close(self):
        from . import torch as vt
        from ._offload import OffloadError

        if self._closed:
            return
        if self._failure is not None:
            raise OffloadError(
                "automatic execution custody remains retained"
            ) from self._failure
        self.stop()
        owner = self._owner_ref()
        for state in self._states.values():
            if state.cache is not None:
                state.cache.close()  # Outside model/admission locks; drains callbacks.
            if state.native_key is not None:
                vt.phase_forget(owner._owner.device.index, state.native_key)
                state.native_key = None
            state.key = None
        self._closed = True
