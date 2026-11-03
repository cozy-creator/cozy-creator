"""Bounded host scalars. No allocation decisions, completion probes or retries."""
from __future__ import annotations

import array
import json
import os
import sys
import threading
import weakref
import time
from types import CodeType

FIELDS = (
    'seq', 'monotonic_ns', 'thread_id', 'event', 'boundary_id',
    'dispatch_attempt', 'segment_call', 'segment_id', 'graph_object_id',
    'plan_handle', 'parent_plan_handle', 'pool_a', 'pool_b', 'stream',
    'use_handle', 'cache_entry_id', 'native_cache_owner_id', 'tail_event_id',
    'status', 'bytecode_offset', 'native_marker', 'native_ordinal',
)
EVENTS = {name: i + 1 for i, name in enumerate((
    'dispatch_enter', 'dispatch_exit', 'capture_enter', 'capture_exit',
    'graph_enter', 'graph_exit', 'prepare_use_enter', 'prepare_use_exit',
    'wait_ready_enter', 'wait_ready_exit', 'replay_enter', 'replay_exit',
    'tails_enter', 'tails_exit', 'release_enter', 'release_exit',
    'restore_enter', 'restore_exit', 'callback_enter', 'callback_exit',
    'definition_close_enter', 'definition_close_exit', 'phase_enter',
    'phase_exit', 'component_exit_enter', 'component_exit_exit', 'host_probe',
    'acquire_enter', 'acquire_exit', 'select_enter', 'select_exit',
    'input_copy_enter', 'input_copy_exit', 'output_clone_enter', 'output_clone_exit',
))}
STATUS = {'call': 1, 'normal': 2, 'unwind': 3, 'yield': 4, 'declined': 5}
UNAVAILABLE = b'VARENA_HOST_STAGE_UNAVAILABLE {"available":false,"scope":"host_profile_lifecycle"}\n'


def _handle(obj):
    value = getattr(obj, '_handle', 0) if obj is not None else 0
    return value if type(value) is int and value >= 0 else 0


class Ring:
    """Primitive data and code identities; graph registry keys are weak only."""
    def __init__(self, *, capacity=8192, proof=False, armed=False, native=None):
        if type(capacity) is not int or capacity <= 0:
            raise ValueError('positive integer ring capacity required')
        self.capacity = capacity
        self.data = array.array('Q', [0]) * (capacity * len(FIELDS))
        self.lock = threading.Lock()
        self.size = 0
        self.lost = 0
        self.codes = {}
        self.names = {}
        self.modules = {}
        self.controllers = set()
        self.devices = {}
        self.identity_unavailable = {}
        self.attempts = {}
        self.entries = {}
        self.native_ids = {}
        self.contexts = {}
        self.last_context = {}
        self.following_returned = False
        self.calls = {}
        self.segment_calls = {}
        self.captures = set()
        self.replays = set()
        self.uncertain = False
        self.running = False
        self.stopped = False
        self.armed = armed
        self.window_done = False
        self.proof = proof
        self.main_thread = threading.get_ident()
        self.metadata = {}
        self.native = native
        self.hook = None
        self.graph_tokens = weakref.WeakKeyDictionary()
        self.next_graph = 1
        self.bindings = {}

    def bind(self, code: CodeType, kind: str):
        self.codes[code] = kind

    def register(self, controller_id, module_id, name):
        if type(name) is not str or type(controller_id) is not int or type(module_id) is not int:
            raise TypeError('primitive boundary registration required')
        boundary = next((n for n, value in self.names.items() if value == name), None)
        if boundary is None:
            boundary = len(self.names) + 1
            self.names[boundary] = name
        self.modules[(controller_id, module_id)] = boundary
        return boundary

    def install(self):
        from monitoring_hook import InterpreterHook
        self.hook = InterpreterHook(self)
        self.hook.install()
        self.running = True

    def stop(self):
        if self.hook is None or not self.hook.owned():
            self.uncertain = True
        else:
            self.hook.stop()
        if self.calls or any(self.contexts.values()):
            self.uncertain = True
        self.stopped = not (self.uncertain or self.captures or self.replays)
        self.running = False

    def graph_identity(self, segment):
        token = self.graph_tokens.get(segment)
        if token is None:
            token = self.next_graph
            self.next_graph += 1
            self.graph_tokens[segment] = token
        return token

    def _emit(self, name, context, status, offset):
        if not self.armed or self.window_done:
            return
        if not self.lock.acquire(blocking=False):
            self.lost += 1
            return
        try:
            if self.size == self.capacity:
                self.lost += 1
                return
            values = (
                self.size + 1, time.monotonic_ns(), threading.get_ident(), EVENTS[name],
                context.get('boundary_id', 0), context.get('dispatch_attempt', 0),
                context.get('segment_call', 0), context.get('segment_id', 0),
                context.get('graph_object_id', 0), context.get('plan_handle', 0),
                context.get('parent_plan_handle', 0), context.get('pool_a', 0),
                context.get('pool_b', 0), context.get('stream', 0),
                context.get('use_handle', 0), context.get('cache_entry_id', 0),
                self.native_ids.get(context.get('cache_entry_id', 0), 0),
                context.get('tail_event_id', 0), status, max(0, offset),
                0, 0,
            )
            if any(type(v) is not int or not 0 <= v < 2**64 for v in values):
                self.lost += 1
                return
            if self.native is not None:
                marker = native_marker(name, status, self.names.get(context.get('boundary_id')), context.get('dispatch_attempt', 0))
                if marker:
                    missing = {}
                    if not context.get('segment_id'):
                        missing['graph_identity'] = 'no bound definition at this stage'
                    if not context.get('plan_handle'):
                        missing['plan_handle'] = 'no ABI Plan handle at this stage'
                    if not (context.get('pool_a') or context.get('pool_b')):
                        missing['pool'] = 'pool not established at this stage'
                    if context.get('device', -1) < 0:
                        missing['device'] = 'device unavailable in context'
                    if missing:
                        self.identity_unavailable[self.size + 1] = missing
                    ordinal = self.native.marker(marker, context)
                    if not ordinal:
                        self.lost += 1
                        return
                    values = values[:-2] + (marker, ordinal)
            start = self.size * len(FIELDS)
            for i in range(1, len(FIELDS)):
                self.data[start + i] = values[i]
            self.data[start] = values[0]  # Commit the row only after its fields.
            self.size += 1
        finally:
            self.lock.release()

    def _control(self, frame, event, status, kind):
        if kind not in ('capture_begin', 'capture_complete', 'graph_scope', 'replay_scope'):
            return
        loc = frame.f_locals
        if kind == 'capture_begin' and event == 'call':
            caller = frame.f_back
            if caller is None or self.codes.get(caller.f_code) != 'graph_scope':
                self.uncertain = True
                return
            p = caller.f_locals['pool']
            self.captures.add((caller.f_locals['device'], p[0], p[1]))
        elif kind == 'capture_begin' and event == 'return' and status == STATUS['normal']:
            if self.native is not None and self._target():
                self.native.stage(3)
        elif kind == 'capture_complete' and event == 'return' and status == STATUS['normal']:
            p = loc['pool']
            self.captures.discard((loc['index'], p[0], p[1]))
            if self.native is not None and self._target():
                self.native.stage(4)
        elif kind == 'graph_scope' and event == 'return' and status != STATUS['yield']:
            if self.captures:
                self.uncertain = True
        elif kind == 'replay_scope':
            if event == 'call':
                self.replays.add(id(frame))
            elif event == 'return' and status != STATUS['yield']:
                self.replays.discard(id(frame))
                if status != STATUS['normal']:
                    self.uncertain = True

    def _target(self):
        context = self._current()
        return self.names.get(context.get('boundary_id')) == 'unet.up_blocks.0' and context.get('dispatch_attempt') in (2, 3)

    def _current(self):
        stack = self.contexts.get(threading.get_ident(), ())
        return dict(stack[-1][1]) if stack else dict(self.last_context.get(threading.get_ident(), {}))

    def _definition(self, context, definition, segment=None, epoch=None):
        if definition is None:
            return context
        p = definition.pool
        index = definition.owner._owner.device.index if definition.owner is not None else None
        context['device'] = index if type(index) is int else -1
        context.update(plan_handle=_handle(definition.plan), pool_a=p[0], pool_b=p[1])
        if segment is not None:
            context['segment_id'] = self.graph_identity(segment)
            graph = getattr(segment, '_graph', None)
            tail = getattr(segment, '_tail', None)
            context['graph_object_id'] = id(graph) if graph is not None else 0
            context['tail_event_id'] = id(tail) if tail is not None else 0
        if epoch is not None:
            if epoch.used is not None:
                context['use_handle'] = _handle(epoch.used)
            context['parent_plan_handle'] = _handle(epoch.parent_plan)
            if epoch.stream is not None:
                context['stream'] = epoch.stream.cuda_stream
        return context

    def observe(self, frame, event, result, status):
        if event not in ('call', 'return'):
            return
        kind = self.codes.get(frame.f_code)
        if kind is None:
            return
        try:
            if event == 'return' and status == STATUS['normal'] and result is False:
                status = STATUS['declined']
            self._control(frame, event, status, kind)
            if kind in ('capture_begin', 'capture_complete', 'graph_scope', 'replay_scope'):
                return
            if kind == 'binding_scope':
                if self.native is not None and self._target():
                    if event == 'call':
                        self.native.stage(3 if self.captures else 2)
                    elif event == 'return' and status != STATUS['yield'] and not self.captures:
                        self.native.stage(4)
                return
            # Only cached scalar context can be observed while exclusion is active.
            if self.captures or self.replays or self.uncertain:
                if kind == 'replay':
                    self._emit('replay_enter' if event == 'call' else 'replay_exit', self._current(), status, frame.f_lasti)
                return
            if self.window_done and kind != 'cache_facts':
                return
            loc = frame.f_locals
            fid = id(frame)
            if kind == 'allocator_bind':
                if self.native is not None and event == 'return' and status == STATUS['normal'] and not self.native.install_after_allocator():
                    self.lost += 1
                return
            if kind == 'weight_entries':
                if event == 'return' and status == STATUS['normal']:
                    from safe_bindings import primitives
                    token = self.graph_identity(loc['self'])
                    try:
                        self.bindings.setdefault(token, {})['weights'] = {
                            'region_handles': primitives(loc['snapshot']['region_handles']),
                            'selected_entries': primitives(result),
                            'scope': 'existing product census return, sampled/not atomic'}
                    except BaseException as error:
                        self.bindings.setdefault(token, {})['weights'] = {'available': False, 'reason': type(error).__name__}
                return
            if kind == 'controller_init':
                if event == 'return' and status == STATUS['normal']:
                    obj = loc['self']
                    self.controllers.add(id(obj))
                    owner = obj._owner_ref()
                    index = owner._owner.device.index if owner is not None else None
                    self.devices[id(obj)] = index if type(index) is int else -1
                    for state in obj._states.values():
                        self.register(id(obj), id(state.boundary.module), state.boundary.name)
                return
            if kind == 'cache_init':
                if event == 'return' and status == STATUS['normal']:
                    context = self._current()
                    entry = id(loc['self'])
                    self.entries[entry] = context.get('boundary_id', 0)
                    stack = self.contexts.get(threading.get_ident(), ())
                    if stack:
                        stack[-1][1]['cache_entry_id'] = entry
                return
            if kind == 'cache_facts':
                if event == 'return' and status == STATUS['normal'] and type(result) is dict:
                    native = result.get('native')
                    if type(native) is dict and type(native.get('id')) is int:
                        self.native_ids[id(loc['self'])] = native['id']
                return
            if kind == 'dispatch':
                if event == 'call':
                    obj, module = loc['self'], loc['module']
                    boundary = self.modules.get((id(obj), id(module)))
                    if boundary is None or self.contexts.get(threading.get_ident()):
                        return
                    attempt = self.attempts.get(boundary, 0) + 1
                    self.attempts[boundary] = attempt
                    state = obj._states[id(module)]
                    context = {'boundary_id': boundary, 'dispatch_attempt': attempt,
                               'device': self.devices.get(id(obj), -1),
                               'cache_entry_id': id(state.cache) if state.cache is not None else 0}
                    scope = loc.get('scope')
                    if scope is not None and scope.stream is not None:
                        context['stream'] = scope.stream.cuda_stream
                    if self.following_returned and self.names[boundary] == 'unet.up_blocks.2' and attempt == 3:
                        self._emit('dispatch_enter', context, status, frame.f_lasti)
                        self.window_done = True
                        if self.native is not None:
                            self.native.close_window()
                        return
                    self.contexts.setdefault(threading.get_ident(), []).append((fid, context))
                    self.calls[fid] = ('dispatch', context)
                    if self.names[boundary] == 'unet.up_blocks.0' and attempt == 2:
                        self.armed = True
                        if self.native is not None:
                            self.native.open_window(context)
                    self._emit('dispatch_enter', context, status, frame.f_lasti)
                elif fid in self.calls:
                    _, context = self.calls.pop(fid)
                    self._emit('dispatch_exit', context, status, frame.f_lasti)
                    if self.native is not None and self.names.get(context.get('boundary_id')) == 'unet.up_blocks.0':
                        self.native.stage(0)
                    self.contexts[threading.get_ident()].pop()
                    self.last_context[threading.get_ident()] = dict(context)
                    if self.names.get(context['boundary_id']) == 'unet.up_blocks.1' and context['dispatch_attempt'] == 3 and status == STATUS['normal']:
                        self.following_returned = True
                return
            if event == 'call':
                context = self._current()
                obj = loc.get('self')
                if kind in ('capture', 'graph'):
                    if kind == 'graph':
                        context = self._definition(context, obj._definition, segment=obj)
                        token = context['segment_id']
                        count = self.segment_calls.get(token, 0) + 1
                        self.segment_calls[token] = count
                        context['segment_call'] = count
                    else:
                        context['segment_id'] = self.graph_identity(obj)
                    if loc.get('stream') is not None:
                        context['stream'] = loc['stream'].cuda_stream
                elif kind in ('acquire', 'release', 'select', 'restore'):
                    context = self._definition(context, obj.definition, epoch=obj)
                elif kind in ('tails', 'definition_close'):
                    context = self._definition(context, obj._definition, segment=obj)
                elif kind == 'callback':
                    context = {'cache_entry_id': id(obj), 'boundary_id': self.entries.get(id(obj), 0)}
                    segment = obj.segment
                    if segment is not None:
                        context = self._definition(context, segment._definition, segment=segment)
                    self.contexts.setdefault(threading.get_ident(), []).append((fid, context))
                elif kind in ('prepare_use', 'wait_ready'):
                    if kind == 'prepare_use': context['stream'] = loc['stream']
                    else: context['use_handle'] = _handle(obj)
                self.calls[fid] = (kind, context)
                stack = self.contexts.get(threading.get_ident(), ())
                if stack:
                    stack[-1][1].update(context)
            else:
                item = self.calls.get(fid)
                if item is None:
                    return
                _, context = item
                if kind == 'capture' and status == STATUS['normal']:
                    from safe_bindings import definition_binding
                    token = self.graph_identity(loc['self'])
                    try:
                        self.bindings.setdefault(token, {})['definition'] = definition_binding(loc['self'])
                    except BaseException as error:
                        self.bindings.setdefault(token, {})['definition'] = {'available': False, 'reason': type(error).__name__}
                if status != STATUS['yield']:
                    self.calls.pop(fid, None)
                if kind == 'acquire':
                    context = self._definition(context, loc['self'].definition, epoch=loc['self'])
                elif kind == 'prepare_use' and status == STATUS['normal']:
                    context['use_handle'] = _handle(result)
                elif kind in ('capture', 'tails', 'graph'):
                    obj = loc['self']
                    definition = getattr(obj, '_definition', None)
                    if definition is not None:
                        context = self._definition(context, definition, segment=obj)
                stack = self.contexts.get(threading.get_ident(), ())
                if stack:
                    stack[-1][1].update(context)
            names = {'capture': 'capture', 'graph': 'graph', 'prepare_use': 'prepare_use',
                     'wait_ready': 'wait_ready', 'acquire': 'acquire', 'release': 'release',
                     'select': 'select', 'restore': 'restore', 'tails': 'tails', 'callback': 'callback',
                     'definition_close': 'definition_close', 'phase': 'phase',
                     'component_exit': 'component_exit', 'probe': 'host_probe'}
            name = names.get(kind)
            if name:
                name = name if kind == 'probe' else name + ('_enter' if event == 'call' else '_exit')
                self._emit(name, context, status, frame.f_lasti)
            if kind == 'callback' and event == 'return':
                self.contexts[threading.get_ident()].pop()
        except BaseException:
            # Diagnostic failure must never replace a product return/exception.
            self.uncertain = True

    @property
    def available(self):
        owned = self.hook is not None and self.hook.owned() if self.running else self.stopped
        return owned and not (self.uncertain or self.captures or self.replays or self.lost)

    @property
    def safe_to_read(self):
        return self.stopped and not (self.uncertain or self.captures or self.replays)

    def write(self, path):
        if not self.available:
            try:
                os.write(2, UNAVAILABLE)
            except OSError:
                pass
            return False
        # No product or native reads here. The buffer contains scalars only.
        values = self.data[:self.size * len(FIELDS)].tolist()
        rows = [values[n:n + len(FIELDS)] for n in range(0, len(values), len(FIELDS))]
        payload = dict(format='varena-host-stage-v2', available=True,
                       host_control_only=self.proof,
                       normal_return_is_gpu_completion=False, fields=FIELDS,
                       events=EVENTS, statuses=STATUS, rows=rows, lost=self.lost,
                       names=self.names, native_cache_ids=self.native_ids,
                       metadata=self.metadata, window_done=self.window_done, bindings=self.bindings,
                       identity_unavailable=self.identity_unavailable,
                       graph_identity_kind='weak-key diagnostic GraphSegment token, not CUDA handle')
        try:
            with open(path, 'x', encoding='utf8') as output:
                json.dump(payload, output, separators=(',', ':'))
                output.write('\n')
        except BaseException:
            self.uncertain = True
            try:
                os.write(2, UNAVAILABLE)
            except OSError:
                pass
            return False
        return True



def native_marker(event, status, boundary, invocation):
    """Agreed namespace; no marker is inferred from bytes or pointer proximity."""
    if event.startswith('callback_'):
        if status == STATUS['declined']:
            return 203
        return 200 + (0 if status == STATUS['call'] else 2 if status == STATUS['unwind'] else 1)
    if boundary == 'unet.up_blocks.1' and invocation == 3 and event.startswith('dispatch_'):
        return 170 + (0 if status == STATUS['call'] else 2 if status == STATUS['unwind'] else 1)
    if boundary != 'unet.up_blocks.0' or invocation not in (2, 3):
        return 0
    bases = {'dispatch': 100, 'prepare_use': 110, 'wait_ready': 120,
             'input_copy': 130, 'replay': 140, 'output_clone': 150,
             'release': 160, 'select': 180, 'restore': 190}
    stem = event.removesuffix('_enter').removesuffix('_exit')
    base = bases.get(stem)
    return 0 if base is None else base + (0 if status == STATUS['call'] else 2 if status == STATUS['unwind'] else 1)
