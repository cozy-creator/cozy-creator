"""Segmented immutable offload graphs with per-execution checked weight exports."""

from __future__ import annotations

import contextvars
import inspect
import os
import threading
from contextlib import contextmanager
from dataclasses import dataclass, field

import torch
from torch import nn
from torch.multiprocessing.reductions import StorageWeakRef
from torch.utils._python_dispatch import TorchDispatchMode
from torch.utils._pytree import tree_flatten, tree_map

from .. import _ext
from . import _offload as adapter
from . import api
from . import torch as vt

# Uncertain launch/cleanup never loses exact graph, pool, input, Source or Pin owners.
# No background work or retries are performed by this process-lifetime custody list.
_UNPROVEN = []
_ACTIVE_EXECUTION = contextvars.ContextVar("varena_graph_execution", default=None)


class GraphInvalidated(adapter.OffloadError):
    """The current model no longer selects this immutable graph definition."""


class CaptureOwnershipUncertain(adapter.OffloadError):
    """Capture storage aliases were not retired; exact native owners stay retained."""


def _storage_tokens(exports):
    return tuple({StorageWeakRef(tensor.untyped_storage()) for tensor in exports})


def _live_storages(tokens):
    result = []
    for token in tokens:
        storage = torch.UntypedStorage._new_with_weak_ptr(token.cdata)
        if storage is not None:
            result.append(storage)
    return tuple(result)


def _ordinary(tensor):
    return (
        type(tensor) is torch.Tensor
        and tensor.layout == torch.strided
        and not tensor.is_conj()
        and not tensor.is_neg()
    )


def _tree_signature(tree):
    values, spec = tree_flatten(tree)
    descriptors = []
    storages = set()
    for value in values:
        if isinstance(value, torch.Tensor):
            if (
                not _ordinary(value)
                or value.requires_grad
                or value.storage_offset()
                or torch._debug_has_internal_overlap(value)
            ):
                raise adapter.OffloadError(
                    "graph inputs require ordinary nonoverlapping tensors at storage offset zero"
                )
            storage = value.untyped_storage()._cdata
            if storage in storages:
                raise adapter.OffloadError(
                    "aliased graph input leaves need an explicit adapter"
                )
            storages.add(storage)
            descriptors.append(
                (
                    "tensor",
                    tuple(value.shape),
                    tuple(value.stride()),
                    value.dtype,
                    value.device,
                )
            )
        elif type(value) in (str, int, float, bool, type(None)):
            descriptors.append(("scalar", type(value), value))
        else:
            raise adapter.OffloadError(
                "graph input leaves must be ordinary tensors or immutable scalars"
            )
    return spec, descriptors


class _BufferGuard(TorchDispatchMode):
    def __init__(self, buffers):
        self.buffers = buffers

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        if func._schema.name == "aten::set_" and any(
            isinstance(value, adapter._Weight)
            for value in tree_flatten((args, kwargs))[0]
        ):
            raise adapter.OffloadError(
                "capture cannot reassign ordinary storage to managed weights"
            )
        if func.namespace != "aten":
            raise adapter.OffloadError(
                f"graph custom operator needs an adapter: {func}"
            )
        for index, argument in enumerate(func._schema.arguments):
            value = (
                args[index]
                if index < len(args)
                else kwargs.get(argument.name, argument.default_value)
            )
            if argument.name in ("training", "train") and value is True:
                raise adapter.OffloadError(
                    "training is not a graph inference operation"
                )
            if argument.alias_info is not None and argument.alias_info.is_write:
                for leaf in tree_flatten(value)[0]:
                    if type(leaf) is torch.Tensor and any(
                        torch._C._is_alias_of(leaf, buffer) for buffer in self.buffers
                    ):
                        raise adapter.OffloadError(
                            "graph preparation cannot mutate module buffers or inputs"
                        )
        return func(*args, **kwargs)


class _CaptureBindings:
    """Frozen ordinary tensors exist only for the capture epoch, never its cache."""

    def __init__(self, owner, module, prepared, stream, inputs=()):
        self.owner, self.module, self.prepared, self.stream = (
            owner,
            module,
            prepared,
            stream,
        )
        self.modules = {id(child) for child in module.modules()}
        self.buffers = (
            tuple(module.buffers())
            + tuple(prepared.values())
            + tuple(v for v in tree_flatten(inputs)[0] if type(v) is torch.Tensor)
        )

    def validate_module(self, owner, module):
        if owner is not self.owner or id(module) not in self.modules:
            raise adapter.OffloadError(
                "graph capture crossed its frozen module boundary"
            )

    def tensor(self, weight):
        version = weight._version_owner
        if version not in self.prepared:
            raise adapter.OffloadError(
                "graph capture read an undeclared immutable version"
            )
        version.byte_offset(weight._shape, weight._strides, weight._offset)
        return self.prepared[version].as_strided(
            weight._shape, weight._strides, weight._offset
        )

    @contextmanager
    def execution(self, owner):
        if owner is not self.owner._owner:
            raise adapter.OffloadError("graph capture crossed offload owners")
        yield self.stream

    @contextmanager
    def scope(self):
        if adapter._GRAPH_BINDING.get() is not None:
            raise adapter.OffloadError("graph binding scopes cannot nest")
        token = adapter._GRAPH_BINDING.set(self)
        try:
            with _BufferGuard(self.buffers):
                yield
        finally:
            adapter._GRAPH_BINDING.reset(token)


@dataclass
class _Definition:
    owner: object
    module: nn.Module
    versions: tuple
    regions: tuple
    plan: object
    phase: str
    pool: tuple
    parameter_state: tuple
    buffer_state: tuple
    signature: object
    buffer_owners: tuple = ()
    configuration: tuple = ()
    streams: list = field(default_factory=list)
    leased_versions: tuple = ()
    static_tree: object = None
    outputs: object = None
    footprint: object = None


@dataclass
class _Epoch:
    definition: _Definition
    stream: object
    used: object = None
    exports: list = field(default_factory=list)
    prepared: dict = field(default_factory=dict)
    pool_reference: bool = False
    inputs: object = None
    outputs: object = None
    release_attempted: bool = False
    released: bool = False
    tails_attempted: bool = False
    finished: bool = False
    capture_storages: tuple = ()
    retained_storages: tuple = ()
    parent_plan: object = None
    parent_phase: str | None = None
    parent_suspended: bool = False
    intent_active: bool = False
    intent_restored: bool = False
    intent_error: object = None

    def select_intent(self):
        owner = self.definition.owner
        if owner._intent_frames:
            self.parent_plan = owner._intent_plan
            self.parent_phase = owner._intent_frames[-1]
            if self.parent_plan is None:
                raise adapter.OffloadError("enclosing intent has no retained Plan")
            try:
                self.parent_plan.deactivate()
            except BaseException as error:
                self.intent_error = error
                owner._intent_failed = True
                raise
            self.parent_suspended = True
        self.definition.plan.activate(self.definition.phase)
        self.intent_active = True

    def restore_intent(self):
        if self.intent_restored:
            return
        if self.intent_error is not None:
            raise self.intent_error
        try:
            if self.intent_active:
                self.definition.plan.deactivate()
                self.intent_active = False
            if self.parent_suspended:
                self.parent_plan.activate(self.parent_phase)
                self.parent_suspended = False
        except BaseException as error:
            self.intent_error = error
            if self.parent_plan is not None:
                self.definition.owner._intent_failed = True
            raise
        self.intent_restored = True

    def acquire(self, *, capture):
        definition = self.definition
        if capture:
            for version in definition.versions:
                if version not in definition.leased_versions:
                    self.prepared[version] = torch.empty(
                        0,
                        dtype=getattr(torch, version.source.dtype),
                        device=definition.owner._owner.device,
                    )
        if not definition.regions:
            return
        self.used = definition.owner._owner.gpu.use(
            definition.regions, self.stream.cuda_stream
        )
        if capture:
            index = {region: i for i, region in enumerate(definition.regions)}
            for version in definition.versions:
                if version not in definition.leased_versions:
                    continue
                count = (
                    version.source.nbytes
                    // getattr(torch, version.source.dtype).itemsize
                )
                if count:
                    tensor = self.used.tensor(
                        index[version.region],
                        dtype=version.source.dtype,
                        shape=(count,),
                        byte_offset=version.member_offset,
                        stream=self.stream.cuda_stream,
                    )
                    self.exports.append(tensor)
                    self.prepared[version] = tensor
                else:
                    self.prepared[version] = torch.empty(
                        0,
                        dtype=getattr(torch, version.source.dtype),
                        device=definition.owner._owner.device,
                    )
            # Published ordering alone is not capture admission: retain all
            # ordinary typed bindings while scoped producer retirement settles.
            self.used.wait_ready()
        else:
            # One valid storage export pins the entire Region and supplies its
            # checked consumer family; it need not duplicate every typed view.
            for index in range(len(definition.regions)):
                self.exports.append(
                    self.used.tensor(
                        index, dtype="uint8", shape=(1,), stream=self.stream.cuda_stream
                    )
                )

    def release(self):
        if self.released:
            return
        if self.release_attempted:
            raise adapter.OffloadError(
                "previous epoch release failed; exact owners retained"
            )
        self.release_attempted = True
        for tensor in tree_flatten((self.inputs, self.outputs))[0]:
            if type(tensor) is torch.Tensor and tensor.device.type == "cuda":
                tensor.record_stream(self.stream)
        # Exports remain independently pinned while the original Use closes.
        # Their final deleters supply checked fences/quarantine after all submits.
        for tensor in self.exports:
            tensor.record_stream(self.stream)
        if self.used is not None:
            self.used.close()
            self.used = None
        self.prepared.clear()
        self.exports.clear()
        self.released = True


class GraphSegment:
    """One fixed module graph; model rebinding invalidates new replay selection."""

    def __init__(
        self, owner, module, args, *, kwargs=None, stream, reserve_bytes, pool=None
    ):
        self._pid = os.getpid()
        self._lock = threading.RLock()
        self._closed, self._busy, self._failure = False, False, None
        self._closing, self._closed_facts = False, None
        self._graph, self._definition, self._tail = None, None, None
        if (
            _ACTIVE_EXECUTION.get() is not None
            or adapter._GRAPH_BINDING.get() is not None
        ):
            raise adapter.OffloadError(
                "prepare graphs outside another capture/replay epoch"
            )
        if owner._owner.device.type != "cuda":
            raise adapter.OffloadError("graph capture requires a CUDA offload owner")
        vt._installed()  # Reject inherited owners before any Python/native lock.
        if not hasattr(api.Use, "wait_ready"):
            raise NotImplementedError(
                "graph capture requires scoped native Use.wait_ready"
            )
        if "synchronize" not in inspect.signature(vt.graph).parameters:
            raise NotImplementedError(
                "graph capture requires agreed scoped vt.graph preparation without a context sync"
            )
        if not isinstance(module, nn.Module) or not any(
            module is child
            for root in owner._roots.values()
            for child in root.modules()
        ):
            raise adapter.OffloadError(
                "capture requires a module belonging to this offload owner"
            )
        if (
            stream is None
            or stream.device != owner._owner.device
            or stream.cuda_stream <= 2
        ):
            raise adapter.OffloadError(
                "capture requires an explicit stream on the owner device"
            )
        if not isinstance(reserve_bytes, int) or reserve_bytes < 0:
            raise ValueError(
                "reserve_bytes must be a nonnegative existing pool preparation target"
            )
        if pool is not None:
            raise adapter.OffloadError(
                "shared pools require an ordered graph group; this adapter creates a private pool"
            )
        tree = (tuple(args), dict(kwargs or {}))
        signature = _tree_signature(tree)
        if any(
            isinstance(v, torch.Tensor) and v.device != owner._owner.device
            for v in tree_flatten(tree)[0]
        ):
            raise adapter.OffloadError(
                "graph example tensors must be on the owner device"
            )
        with owner._owner.lock, self._lock, torch.inference_mode():
            owner._check()
            if owner._intent_frames:
                raise adapter.OffloadError(
                    "prepare a graph outside an active declared forward"
                )
            owner._refresh_intent()
            versions = tuple(
                dict.fromkeys(p._version_owner for p in module.parameters())
            )
            leased = tuple(
                dict.fromkeys(
                    p._version_owner for p in module.parameters() if p.numel()
                )
            )
            regions = tuple(dict.fromkeys(v.reservation() for v in leased))
            phase = f"graph:{id(self)}"
            pool = (
                tuple(torch.cuda.graph_pool_handle()) if pool is None else tuple(pool)
            )
            plan = owner._owner.gpu.plan({phase: regions})
            definition = _Definition(
                owner,
                module,
                versions,
                regions,
                plan,
                phase,
                pool,
                self._parameters(module),
                self._buffers(module),
                signature,
                buffer_owners=tuple(buffer.detach() for buffer in module.buffers()),
                leased_versions=leased,
            )
            if any(
                buffer.device != owner._owner.device
                for buffer in definition.buffer_owners
            ):
                plan.close()
                raise adapter.OffloadError(
                    "graph module buffers must be on the owner device"
                )
            definition.configuration = self._configuration(module)
            definition.streams.append(stream)
            self._definition = definition
            graph = self._graph = torch.cuda.CUDAGraph()
            epoch = _Epoch(definition, stream, inputs=tree)
            activated = False
            try:
                plan.activate(phase)
                activated = True
                caller = torch.cuda.current_stream(owner._owner.device)
                with torch.cuda.stream(stream):
                    if caller != stream:
                        stream.wait_stream(caller)
                    epoch.acquire(capture=True)
                    epoch.capture_storages = _storage_tokens(epoch.exports)
                    definition.static_tree = tree_map(self._static_input, tree)
                    binding = _CaptureBindings(
                        owner, module, epoch.prepared, stream, definition.static_tree
                    )
                    with binding.scope():
                        module(*definition.static_tree[0], **definition.static_tree[1])
                    self._validate_model()
                    stream.synchronize()  # Checked preparation, never capture completion.
                    definition.footprint = vt.capture_footprint(
                        owner._owner.device, pool
                    )
                    with (
                        vt.graph(
                            graph,
                            pool=pool,
                            stream=stream,
                            reserve_bytes=reserve_bytes,
                            footprint=definition.footprint,
                            synchronize=False,
                        ),
                        binding.scope(),
                    ):
                        definition.outputs = module(
                            *definition.static_tree[0], **definition.static_tree[1]
                        )
                    self._validate_model()
                    outputs = tree_flatten(definition.outputs)[0]
                    if any(not _ordinary(value) for value in outputs):
                        raise adapter.OffloadError(
                            "graph outputs must be ordinary independent working tensors"
                        )
                    if any(
                        torch._C._is_alias_of(value, tensor)
                        for value in outputs
                        for tensor in epoch.exports
                    ):
                        raise adapter.OffloadError(
                            "captured weight aliases cannot become ordinary graph outputs"
                        )
                    epoch.release()
                    # Track C10 Storage families, not one Tensor's Python weakref.
                    # Reacquiring the same live Storage preserves its actual
                    # TensorExport/Pin if a detached or hidden alias escaped.
                    epoch.retained_storages = _live_storages(epoch.capture_storages)
                    if epoch.retained_storages:
                        raise CaptureOwnershipUncertain(
                            "capture storage aliases remain; exact exports retained, streamed classification refused"
                        )
                plan.deactivate()
                activated = False
                observed = self._weight_entries()
                if any(row["pins"] or row["unjoined"] for row in observed):
                    raise CaptureOwnershipUncertain(
                        "capture leaves physical weight Pins; streamed graph classification refused"
                    )
                # Resource does not reference graph/segment. Native pool/weight
                # tails protect submitted GPU work if this definition is dropped.
                graph.retain_object(definition, synchronize_before_release=False)
            except BaseException as error:
                if isinstance(error, CaptureOwnershipUncertain) or (
                    epoch.release_attempted and not epoch.released
                ):
                    self._retain_failure(epoch, error)
                    raise
                try:
                    if torch.cuda.is_current_stream_capturing():
                        raise adapter.OffloadError(
                            "capture did not end; exact epoch retained"
                        )
                    graph.reset()
                    epoch.release()
                    if activated:
                        plan.deactivate()
                    plan.close()
                except BaseException as cleanup:
                    self._retain_failure(epoch, cleanup)
                    raise cleanup from error
                self._closed = True
                raise

    @staticmethod
    def _parameters(module):
        return tuple(
            (
                name,
                id(p),
                id(p._version_owner),
                p.dtype,
                tuple(p.shape),
                tuple(p.stride()),
                p._offset,
            )
            for name, p in module.named_parameters(remove_duplicate=False)
        )

    @staticmethod
    def _buffers(module):
        return tuple(
            (
                name,
                id(buffer),
                buffer.untyped_storage()._cdata,
                buffer.dtype,
                tuple(buffer.shape),
                tuple(buffer.stride()),
                buffer.storage_offset(),
            )
            for name, buffer in module.named_buffers(remove_duplicate=False)
        )

    @staticmethod
    def _configuration(module):
        rows = []
        for child in module.modules():
            values = tuple(
                sorted(
                    (key, value)
                    for key, value in child.__dict__.items()
                    if type(value)
                    in (str, int, float, bool, type(None), torch.dtype, torch.device)
                )
            )
            forward = child.forward
            rows.append((id(child), id(getattr(forward, "__func__", forward)), values))
        return tuple(rows)

    def _validate_model(self):
        definition = self._definition
        module = definition.module
        if any(
            not isinstance(p, adapter._Weight)
            or p._version_owner.owner is not definition.owner._owner
            for p in module.parameters()
        ):
            raise GraphInvalidated("model parameters changed after capture selection")
        if (
            self._parameters(module) != definition.parameter_state
            or self._buffers(module) != definition.buffer_state
            or self._configuration(module) != definition.configuration
        ):
            raise GraphInvalidated(
                "model version/layout/configuration changed; prepare a new graph"
            )
        if any(child.training for child in module.modules()):
            raise GraphInvalidated("module training state changed")

    @staticmethod
    def _static_input(value):
        if not isinstance(value, torch.Tensor):
            return value
        result = torch.empty_strided(
            value.shape, value.stride(), dtype=value.dtype, device=value.device
        )
        result.copy_(value)
        return result

    def _weight_entries(self):
        definition = self._definition
        snapshot = _ext._varena_public_residency_snapshot(
            definition.owner._owner.gpu._handle, [r._handle for r in definition.regions]
        )
        keys = {(r["arena_id"], r["region_id"]) for r in snapshot["region_handles"]}
        entries = {
            (a["id"], e["id"]): e
            for a in snapshot["arenas"]
            for e in a["entries"]
            if (a["id"], e["id"]) in keys
        }
        if keys != set(entries) or len(keys) != len(definition.regions):
            raise adapter.OffloadError(
                "incomplete Region census cannot classify graph residency"
            )
        return list(entries.values())

    def _validate(self, tree):
        if self._closed or self._closing:
            raise adapter.OffloadError("graph segment is closed")
        if self._failure is not None:
            raise adapter.OffloadError(
                "graph execution ownership is unproven; retire this execution owner"
            )
        definition = self._definition
        definition.owner._check()
        self._validate_model()
        if any(not region._handle for region in definition.regions):
            raise GraphInvalidated("graph Region handle was explicitly closed")
        if definition.footprint.facts()["retired"]:
            raise GraphInvalidated("graph definition was reset")
        if _tree_signature(tree) != definition.signature:
            raise GraphInvalidated("graph input structure/metadata/scalars changed")

    def _retain_failure(self, epoch, error):
        self._failure = str(error)
        if epoch.parent_plan is not None and (
            epoch.parent_suspended or epoch.intent_active
        ):
            epoch.definition.owner._intent_failed = True
        _UNPROVEN.append((self, epoch, error))

    def _check_process(self):
        if self._pid != os.getpid():
            raise adapter.OffloadError("graph definitions cannot be used after fork")

    def _finish_epoch(self, epoch):
        if epoch.finished:
            return
        if epoch.tails_attempted:
            raise adapter.OffloadError(
                "previous epoch tail installation failed; exact owners retained"
            )
        epoch.tails_attempted = True
        definition, stream = epoch.definition, epoch.stream
        with torch.cuda.stream(stream):
            for tensor in tree_flatten(definition.static_tree)[0]:
                if isinstance(tensor, torch.Tensor):
                    tensor.record_stream(stream)
            for tensor in definition.buffer_owners:
                tensor.record_stream(stream)
            for tensor in tree_flatten(definition.outputs)[0]:
                tensor.record_stream(stream)
            vt.record_pool_stream(
                definition.pool, stream, definition.owner._owner.device
            )
            self._tail = torch.cuda.Event()
            self._tail.record(stream)
            epoch.release()
        if epoch.pool_reference:
            _ext._varena_working_pool_release(
                definition.owner._owner.device.index, definition.pool
            )
            epoch.pool_reference = False
        epoch.finished = True

    def __call__(self, *args, stream, **kwargs):
        self._check_process()
        if (
            _ACTIVE_EXECUTION.get() is not None
            or adapter._GRAPH_BINDING.get() is not None
        ):
            raise adapter.OffloadError(
                "host graph segmentation cannot execute inside another capture/replay epoch"
            )
        with self._lock:
            if self._closed or self._closing:
                raise adapter.OffloadError("graph segment is closed")
            definition = self._definition
            device_owner = definition.owner._owner
        tree = (args, kwargs)
        with device_owner.lock, self._lock, torch.inference_mode():
            self._validate(tree)
            if self._busy:
                raise adapter.OffloadError("graph replay admission is already active")
            if (
                stream is None
                or stream.device != definition.owner._owner.device
                or stream.cuda_stream <= 2
            ):
                raise adapter.OffloadError(
                    "replay requires an explicit stream on the owner device"
                )
            caller = torch.cuda.current_stream(definition.owner._owner.device)
            for actual in (caller, stream):
                if actual not in definition.streams:
                    definition.streams.append(actual)
            self._busy = True
            epoch = _Epoch(definition, stream, inputs=tree)
            submitted = False
            execution_token = _ACTIVE_EXECUTION.set(self)
            try:
                epoch.select_intent()
                with torch.cuda.stream(stream):
                    if stream != caller:
                        stream.wait_stream(caller)
                    if self._tail is not None:
                        stream.wait_event(self._tail)
                    epoch.acquire(capture=False)
                    _ext._varena_working_pool_retain(
                        definition.owner._owner.device.index, definition.pool
                    )
                    epoch.pool_reference = True
                    submitted = True  # Copy/launch may enqueue before raising.
                    for source, target in zip(
                        tree_flatten(tree)[0],
                        tree_flatten(definition.static_tree)[0],
                        strict=True,
                    ):
                        if isinstance(source, torch.Tensor):
                            target.copy_(source)
                    # Reuse the existing guard; do not add a public/native replay API.
                    from ._runtime import replay as replay_scope

                    with replay_scope(definition.owner._owner.device.index):
                        vt.replay(self._graph, stream=stream)
                    result = tree_map(lambda tensor: tensor.clone(), definition.outputs)
                    epoch.outputs = result
                    self._finish_epoch(epoch)
                epoch.restore_intent()
                if stream != caller:
                    caller.wait_event(self._tail)
                    for tensor in tree_flatten(result)[0]:
                        tensor.record_stream(caller)
                return result
            except BaseException as error:
                if submitted and epoch.tails_attempted and not epoch.finished:
                    self._retain_failure(epoch, error)
                else:
                    try:
                        if submitted:
                            # A failed copy/hook need not leak known owners. Install
                            # actual checked tails; retain only if that proof fails.
                            self._finish_epoch(epoch)
                        else:
                            epoch.release()
                            if epoch.pool_reference:
                                _ext._varena_working_pool_release(
                                    definition.owner._owner.device.index,
                                    definition.pool,
                                )
                                epoch.pool_reference = False
                    except BaseException as cleanup:
                        self._retain_failure(epoch, cleanup)
                        raise cleanup from error
                if epoch.intent_error is not None:
                    self._retain_failure(epoch, epoch.intent_error)
                elif not epoch.intent_restored:
                    try:
                        epoch.restore_intent()
                    except BaseException as cleanup:
                        self._retain_failure(epoch, cleanup)
                        raise cleanup from error
                raise
            finally:
                self._busy = False
                _ACTIVE_EXECUTION.reset(execution_token)

    def facts(self):
        self._check_process()
        with self._lock:
            if self._closed_facts is not None:
                return dict(self._closed_facts)
            definition = self._definition
            return {
                "phase": definition.phase,
                "region_handles": [r._handle for r in definition.regions],
                "version_dtypes": [v.source.dtype for v in definition.versions],
                "pool": definition.pool,
                "closed": self._closed,
                "unproven": self._failure,
            }

    def close(self):
        self._check_process()
        with self._lock:
            if self._closed:
                return
            definition = self._definition
            device_owner = definition.owner._owner
        with device_owner.lock, self._lock:
            if self._closed:
                return
            if self._busy or self._failure is not None:
                raise adapter.OffloadError(
                    "cannot reset active or unproven graph execution"
                )
            self._closing = True
            try:
                self._graph.reset()
            except BaseException as error:
                self._retain_failure(_Epoch(definition, None), error)
                raise
            definition.plan.close()
            self._closed_facts = {**self.facts(), "closed": True, "region_handles": []}
            definition.plan = None
            definition.outputs = definition.static_tree = None
            definition.buffer_owners = ()
            definition.regions = definition.versions = ()
            definition.leased_versions = ()
            definition.parameter_state = definition.buffer_state = ()
            definition.owner = definition.module = None
            definition.footprint = None
            definition.streams.clear()
            self._definition = self._graph = self._tail = None
            self._closed = True
