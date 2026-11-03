"""Fixture-only scalar profiling of original code; never owns model resources."""

import ctypes
import dis
import json
import threading
import types
import weakref

from capture_pressure_evidence import ReturnEvidence


MARKERS = {
    100: "dispatch_enter", 101: "dispatch_return", 102: "dispatch_error",
    110: "use_prepare_enter", 111: "use_prepare_return", 112: "use_prepare_error",
    120: "wait_ready_enter", 121: "wait_ready_return", 122: "wait_ready_error",
    130: "input_copy_enter", 131: "input_copy_return", 132: "input_copy_error",
    140: "replay_enter", 141: "replay_return", 142: "replay_error",
    150: "output_clone_enter", 151: "output_clone_return", 152: "output_clone_error",
    160: "epoch_release_enter", 161: "epoch_release_return", 162: "epoch_release_error",
    170: "following_boundary_enter", 171: "following_boundary_return", 172: "following_boundary_error",
    180: "select_intent_enter", 181: "select_intent_return", 182: "select_intent_error",
    190: "restore_intent_enter", 191: "restore_intent_return", 192: "restore_intent_error",
    200: "cache_retire_enter", 201: "cache_retire_return", 202: "cache_retire_error",
    203: "cache_retire_declined",
}
LIMIT = 4096


def bind_native(native):
    native.allocation_trace_install.restype = ctypes.c_int
    native.allocation_trace_error.restype = ctypes.c_char_p
    native.allocation_trace_workspace_cache_enabled.argtypes = []
    native.allocation_trace_workspace_cache_enabled.restype = ctypes.c_int
    native.allocation_trace_window.argtypes = [ctypes.c_int]
    native.allocation_trace_window.restype = ctypes.c_int
    native.allocation_trace_stage.argtypes = [ctypes.c_int]
    native.allocation_trace_stage.restype = None
    native.allocation_trace_dump.restype = ctypes.c_char_p
    native.allocation_trace_marker.argtypes = [
        ctypes.c_uint32, ctypes.c_uint32, ctypes.c_int32,
        ctypes.c_uint64, ctypes.c_uint64, ctypes.c_uint64,
        ctypes.c_uint64, ctypes.c_uint64,
    ]
    native.allocation_trace_marker.restype = ctypes.c_uint64


def returns(code):
    return frozenset(i.offset for i in dis.get_instructions(code)
                     if i.opname in ("RETURN_VALUE", "RETURN_CONST"))


class FrontendTrace(ReturnEvidence):
    """One per-thread profile owner, preserving ReturnEvidence's exclusion gate.

    Persistent state contains code objects, weak Definition references, and bounded
    scalar records only. Frames, tensors, storages, epochs, Uses and owners are
    borrowed during callbacks and never retained. Native Rust-created callback
    threads are outside this profile's coverage.
    """

    def __init__(self, native, torch, vt, auto, graph, api, output, manifest):
        self.native, self.torch, self.auto, self.graph = native, torch, auto, graph
        self.output, self.manifest = output, manifest
        self.stage = 0
        self.invocation = 0
        self.boundary = None
        self.native_key = None
        self.device = -1
        self.window = False
        self.complete = False
        self.await_following = False
        self.dispatch_frame = None
        self.following = False
        self.host_rows, self.host_errors, self.identities = [], [], []
        self.pending = {}
        self.definitions = {}
        self.next_identity = 1
        self.dispatch_code = auto._AutomaticGraphs.dispatch.__code__
        self.graph_codes = {graph.GraphSegment.__init__.__code__, graph.GraphSegment.__call__.__code__}
        self.codes = {
            api.Gpu.prepare_use.__code__: 110,
            api.Use.wait_ready.__code__: 120,
            vt.replay.__code__: 140,
            graph._Epoch.release.__code__: 160,
            graph._Epoch.select_intent.__code__: 180,
            graph._Epoch.restore_intent.__code__: 190,
            auto._GraphCacheEntry._shrink.__code__: 200,
        }
        self.return_offsets = {code: returns(code) for code in (*self.codes, self.dispatch_code)}
        self.copy_codes = {graph.GraphSegment.__call__.__code__}
        self.clone_codes = {
            code for code in graph.GraphSegment.__call__.__code__.co_consts
            if isinstance(code, types.CodeType) and code.co_name == "<lambda>"
        }
        self.binding_code = graph._CaptureBindings.scope.__wrapped__.__code__
        self.binding_yields = {i.offset for i in dis.get_instructions(self.binding_code)
                               if i.opname == "YIELD_VALUE"}
        super().__init__(
            vt.prepare_pool, auto._Observation.close, vt.phase_stats, self.context,
            capture_begin=torch.cuda.CUDAGraph.capture_begin,
            capture_complete=vt.capture_complete, graph_scope=vt.graph,
            replay_scope=vt.replay_scope,
        )

    def context(self):
        if self.dispatch_frame is None:
            return None
        return {"boundary": self.boundary, "attempt": self.invocation,
                "device": self.device, "stage": self.stage,
                "native_key": self.native_key}

    def set_stage(self, value):
        self.stage = value
        self.native.allocation_trace_stage(value)

    def fail(self, reason):
        if len(self.host_errors) < 64:
            self.host_errors.append(reason)

    def identity(self, frame):
        # Inspect only original graph lifecycle frames. No native queries here.
        definition = stream = None
        cursor = frame
        while cursor is not None:
            local = cursor.f_locals
            if cursor.f_code in self.graph_codes:
                definition = local.get("definition")
                if definition is None:
                    definition = getattr(local.get("self"), "_definition", None)
                stream = local.get("stream")
            elif cursor.f_code is self.dispatch_code:
                controller = local["self"]
                state = controller._states.get(id(local["module"]))
                cache = getattr(state, "cache", None)
                segment = getattr(cache, "segment", None)
                definition = getattr(segment, "_definition", None)
                stream = getattr(local.get("scope"), "stream", None)
            elif cursor.f_code in self.codes:
                epoch = local.get("self")
                if type(epoch) is self.graph._Epoch:
                    definition, stream = epoch.definition, epoch.stream
                elif self.codes[cursor.f_code] == 200:
                    definition = getattr(getattr(epoch, "segment", None), "_definition", None)
            if definition is not None:
                break
            cursor = cursor.f_back
        if definition is None:
            return (-1, 0, 0, 0, 0, 0), "definition_unavailable_at_this_stage"
        address = id(definition)
        previous = self.definitions.get(address)
        if previous is None or previous[0]() is not definition:
            if len(self.definitions) >= 64:
                self.fail("definition_identity_overflow")
                return (-1, 0, 0, 0, 0, 0), "definition_identity_overflow"
            token = self.next_identity
            self.next_identity += 1
            self.definitions[address] = (weakref.ref(definition), token)
            self.identities.append({"graph_identity": token, "python_definition_id": address,
                                    "identity_kind": "weak_definition_control_incarnation"})
        else:
            token = previous[1]
        device = definition.owner._owner.device.index
        device = device if type(device) is int else -1
        actual_stream = int(stream.cuda_stream) if stream is not None else 0
        # _handle is a public-ABI wrapper handle, never a native Plan ID.
        plan = int(definition.plan._handle)
        pool_a, pool_b = definition.pool
        return (device, actual_stream, token, plan, int(pool_a), int(pool_b)), (
            None if actual_stream and plan else "stream_or_plan_handle_unavailable")

    def emit(self, marker, values, reason=None):
        if len(self.host_rows) >= LIMIT:
            self.fail("host_marker_overflow")
            return
        invocation = self.invocation if self.dispatch_frame is not None else 0
        boundary = self.boundary if self.dispatch_frame is not None else None
        ordinal = self.native.allocation_trace_marker(marker, invocation, *values)
        self.host_rows.append({"ordinal": int(ordinal), "marker": marker,
                               "boundary": boundary, "invocation": invocation,
                               "native_key": self.native_key if boundary is not None else None,
                               "python_thread": threading.get_ident(),
                               "identity_unavailable_reason": reason})
        if not ordinal:
            self.fail("native_marker_not_committed")

    def _dispatch(self, frame, event):
        if event == "call":
            if self.complete or self.dispatch_frame is not None or self.auto._ACTIVE_AUTOMATIC.get() is not None:
                return
            controller = frame.f_locals["self"]
            state = controller._states.get(id(frame.f_locals["module"]))
            if state is None:
                return
            name = state.boundary.name
            target = name == "unet.up_blocks.0"
            if target:
                self.invocation += 1
            if not self.await_following and not (target and self.invocation in (2, 3)):
                return
            self.following = self.await_following
            self.boundary = name
            self.native_key = state.native_key if type(state.native_key) is str else None
            self.dispatch_frame = id(frame)
            device = controller._owner_ref()._owner.device.index
            self.device = device if type(device) is int else -1
            if not self.window:
                if self.native.allocation_trace_window(1) != 0:
                    self.fail("native_window_open_failed")
                self.window = True
            self.set_stage(0 if self.following else 4)
            values, reason = self.identity(frame)
            # scope.stream exists before graph definition selection.
            scope = frame.f_locals.get("scope")
            stream = getattr(scope, "stream", None)
            if values[1] == 0 and stream is not None:
                values = (self.device, int(stream.cuda_stream), *values[2:])
            elif values[1] == 0:
                reason = "definition_or_scope_stream_unavailable"
            self.emit(170 if self.following else 100, values, reason)
        elif event == "return" and id(frame) == self.dispatch_frame:
            normal = frame.f_lasti in self.return_offsets[self.dispatch_code]
            state = frame.f_locals.get("state")
            key = getattr(state, "native_key", None)
            self.native_key = key if type(key) is str else None
            values, reason = self.identity(frame)
            self.emit((171 if self.following else 101) + (not normal), values, reason)
            closing = self.following or not normal
            if self.invocation == 3 and not self.following:
                self.await_following = True
            self.set_stage(0)
            self.dispatch_frame = None
            if self.collection_available:
                if closing:
                    if self.native.allocation_trace_window(0) != 0:
                        self.fail("native_window_close_failed")
                    self.window = False
                    self.complete = self.following
                self.save("following-end" if self.following else "dispatch-end")
            else:
                self.report_unavailable()

    def _observe(self, frame, event, result):
        if frame.f_code is self.dispatch_code:
            self._dispatch(frame, event)
        if not self.window:
            return
        selected = self.dispatch_frame is not None and not self.following
        if selected:
            if frame.f_code is self._begin and event == "call":
                self.set_stage(3)
            elif frame.f_code is self._complete and event == "return":
                if frame.f_lasti in self._returns[self._complete]:
                    self.set_stage(4)
            elif frame.f_code is self.binding_code:
                if event == "call" and self.stage != 3:
                    self.set_stage(2)
                elif event == "return" and frame.f_lasti not in self.binding_yields and self.stage == 2:
                    self.set_stage(4)
        marker = self.codes.get(frame.f_code)
        if marker is not None:
            if event == "call" and (selected or marker == 200):
                if len(self.pending) >= 64:
                    self.fail("host_pending_overflow")
                    return
                values, reason = self.identity(frame)
                self.pending[id(frame)] = (marker, values, reason)
                self.emit(marker, values, reason)
            elif event == "return" and id(frame) in self.pending:
                marker, values, reason = self.pending.pop(id(frame))
                normal = frame.f_lasti in self.return_offsets[frame.f_code]
                ending = marker + (1 if normal else 2)
                if marker == 200 and normal and result is False:
                    ending = 203
                self.emit(ending, values, reason)
        if selected and event in ("c_call", "c_return", "c_exception"):
            name = getattr(result, "__name__", None)
            marker = (130 if frame.f_code in self.copy_codes and name == "copy_" else
                      150 if frame.f_code in self.clone_codes and name == "clone" else None)
            if marker is not None:
                key = (id(frame), marker)
                if event == "c_call":
                    values, reason = self.identity(frame)
                    self.pending[key] = (marker, values, reason)
                    self.emit(marker, values, reason)
                elif key in self.pending:
                    marker, values, reason = self.pending.pop(key)
                    self.emit(marker + (1 if event == "c_return" else 2), values, reason)

    def _returned(self, frame, event, result):
        # Keep the original completion classifier authoritative. A profiling return
        # carrying None alone never establishes successful capture/replay exit.
        super()._returned(frame, event, result)
        try:
            self._observe(frame, event, result)
        except BaseException as error:
            self.fail("host_profile:" + type(error).__name__)

    def save(self, name):
        if not self.collection_available:
            self.report_unavailable()
            return
        prefix = self.output / f"up0-trace-{self.invocation}-{name}"
        # Persist the ring BEFORE any fallible phase_stats/read or optional report.
        raw = self.native.allocation_trace_dump()
        if not raw:
            self.fail("native_dump_unavailable")
            return
        prefix.with_suffix(".native.json").write_bytes(raw + b"\n")
        facts = {
            "scope": "host observations only; normal host return is not GPU completion",
            "trace_complete": self.complete, "markers": MARKERS,
            "host_rows": self.host_rows, "host_errors": self.host_errors,
            "identities": self.identities,
            "callback_scope": "current inference thread only; Rust-created Python callback threads unavailable; join native admission records separately",
            "native_plan_id": "unavailable; marker plan_handle is ABI handle only",
            "physical_release_completion": "unavailable; native kind16 is pre-unmap intent",
            "graph_address_census": "unavailable in this host-only first version; no new native metadata queries",
            "manifest": self.manifest,
        }
        prefix.with_suffix(".host.json").write_text(json.dumps(facts) + "\n")
        try:
            returns = self.read(self.native_key, self.device)
            prefix.with_suffix(".returns.json").write_text(json.dumps(returns) + "\n")
        except BaseException as error:
            self.fail("return_evidence_read:" + type(error).__name__)
            prefix.with_suffix(".returns-error.txt").write_text(type(error).__name__ + "\n")

    def finish(self):
        self.stop()
        if not self.collection_available:
            self.report_unavailable()
            return
        try:
            self.set_stage(0)
            if self.window:
                self.native.allocation_trace_window(0)
                self.window = False
            self.save("process-end")
        except BaseException:
            # Never replace the original inference error with diagnostic failure.
            self.report_unavailable()
