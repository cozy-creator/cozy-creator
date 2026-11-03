"""Bounded host IDs only. No allocator queries, completion probes or product retries."""
from __future__ import annotations

import array
import dis
import json
import os
import sys
import threading
import time
from types import CodeType

FIELDS = (
    'seq', 'monotonic_ns', 'thread_id', 'event', 'boundary_id',
    'dispatch_attempt', 'segment_call', 'segment_id', 'graph_object_id',
    'plan_handle', 'parent_plan_handle', 'pool_a', 'pool_b', 'stream',
    'use_handle', 'cache_entry_id', 'native_cache_owner_id', 'tail_event_id',
    'status', 'bytecode_offset',
)
EVENTS = {name: i + 1 for i, name in enumerate((
    'dispatch_enter', 'dispatch_exit', 'capture_enter', 'capture_exit',
    'graph_enter', 'graph_exit', 'prepare_use_enter', 'prepare_use_exit',
    'wait_ready_enter', 'wait_ready_exit', 'replay_enter', 'replay_exit',
    'tails_enter', 'tails_exit', 'release_enter', 'release_exit',
    'restore_enter', 'restore_exit', 'callback_enter', 'callback_exit',
    'definition_close_enter', 'definition_close_exit', 'phase_enter',
    'phase_exit', 'component_exit_enter', 'component_exit_exit', 'host_probe',
    'acquire_enter', 'acquire_exit',
))}
STATUS = {'call': 1, 'normal': 2, 'unwind': 3, 'yield': 4, 'declined': 5}
UNAVAILABLE = b'VARENA_HOST_STAGE_UNAVAILABLE {"available":false,"scope":"host_profile_lifecycle"}\n'


def _handle(obj):
    value = getattr(obj, '_handle', 0) if obj is not None else 0
    return value if type(value) is int and value >= 0 else 0


class Ring:
    """State is code objects, strings and integers; never live execution objects."""
    def __init__(self, *, capacity=8192, proof=False, armed=False):
        if type(capacity) is not int or capacity <= 0:
            raise ValueError('positive integer ring capacity required')
        self.capacity = capacity
        self.data = array.array('Q', [0]) * (capacity * len(FIELDS))
        self.lock = threading.Lock()
        self.size = 0
        self.lost = 0
        self.codes = {}
        self.returns = {}
        self.yields = {}
        self.names = {}
        self.modules = {}
        self.controllers = set()
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
        self.callback_active = 0
        self.main_thread = threading.get_ident()
        self.callback = self.profile
        self.metadata = {}

    def bind(self, code: CodeType, kind: str):
        self.codes[code] = kind
        ops = tuple(dis.get_instructions(code))
        self.returns[code] = frozenset(i.offset for i in ops if i.opname in ('RETURN_VALUE', 'RETURN_CONST'))
        self.yields[code] = frozenset(i.offset for i in ops if i.opname == 'YIELD_VALUE')

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
        if sys.getprofile() is not None or threading.getprofile() is not None:
            raise RuntimeError('diagnostic profile slot already owned')
        if threading.get_ident() != self.main_thread:
            raise RuntimeError('ring installation changed thread')
        sys.setprofile(self.callback)
        self.running = True

    def stop(self):
        if sys.getprofile() is self.callback:
            sys.setprofile(None)
        else:
            self.uncertain = True
        if self.callback_active or self.calls or any(self.contexts.values()) or threading.getprofile() is not None:
            self.uncertain = True
        self.stopped = not (self.uncertain or self.captures or self.replays or self.lost)
        self.running = False

    def _status(self, frame, event, result):
        if event == 'call':
            return STATUS['call']
        if frame.f_lasti in self.yields[frame.f_code]:
            return STATUS['yield']
        if frame.f_lasti not in self.returns[frame.f_code]:
            return STATUS['unwind']
        return STATUS['declined'] if result is False else STATUS['normal']

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
            )
            if any(type(v) is not int or not 0 <= v < 2**64 for v in values):
                self.lost += 1
                return
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
        elif kind == 'capture_complete' and event == 'return' and status == STATUS['normal']:
            p = loc['pool']
            self.captures.discard((loc['index'], p[0], p[1]))
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

    def _current(self):
        stack = self.contexts.get(threading.get_ident(), ())
        return dict(stack[-1][1]) if stack else dict(self.last_context.get(threading.get_ident(), {}))

    def _definition(self, context, definition, segment=None, epoch=None):
        if definition is None:
            return context
        p = definition.pool
        context.update(plan_handle=_handle(definition.plan), pool_a=p[0], pool_b=p[1])
        if segment is not None:
            context['segment_id'] = id(segment)
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

    def profile(self, frame, event, result):
        if event not in ('call', 'return'):
            return
        kind = self.codes.get(frame.f_code)
        if kind is None:
            return
        try:
            status = self._status(frame, event, result)
            self._control(frame, event, status, kind)
            if kind in ('capture_begin', 'capture_complete', 'graph_scope', 'replay_scope'):
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
            if kind == 'controller_init':
                if event == 'return' and status == STATUS['normal']:
                    obj = loc['self']
                    self.controllers.add(id(obj))
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
                               'cache_entry_id': id(state.cache) if state.cache is not None else 0}
                    if self.following_returned and self.names[boundary] == 'unet.up_blocks.2' and attempt == 3:
                        self._emit('dispatch_enter', context, status, frame.f_lasti)
                        self.window_done = True
                        return
                    self.contexts.setdefault(threading.get_ident(), []).append((fid, context))
                    self.calls[fid] = ('dispatch', context)
                    if self.names[boundary] == 'unet.up_blocks.0' and attempt == 2:
                        self.armed = True
                    self._emit('dispatch_enter', context, status, frame.f_lasti)
                elif fid in self.calls:
                    _, context = self.calls.pop(fid)
                    self._emit('dispatch_exit', context, status, frame.f_lasti)
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
                        count = self.segment_calls.get(id(obj), 0) + 1
                        self.segment_calls[id(obj)] = count
                        context['segment_call'] = count
                    else:
                        context['segment_id'] = id(obj)
                    if loc.get('stream') is not None:
                        context['stream'] = loc['stream'].cuda_stream
                elif kind in ('acquire', 'release', 'restore'):
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
                     'restore': 'restore', 'tails': 'tails', 'callback': 'callback',
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
        owned = sys.getprofile() is self.callback if self.running else self.stopped
        return owned and threading.getprofile() is None and not (self.uncertain or self.captures or self.replays or self.lost or self.callback_active)

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
        payload = dict(format='varena-host-stage-v1', available=True,
                       host_control_only=self.proof,
                       normal_return_is_gpu_completion=False, fields=FIELDS,
                       events=EVENTS, statuses=STATUS, rows=rows, lost=self.lost,
                       names=self.names, native_cache_ids=self.native_ids,
                       metadata=self.metadata, window_done=self.window_done)
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


def callback_attachment(ring, original):
    """Scope a foreign thread's profile; preserve the original callable once."""
    def observed(*args, **kwargs):
        if not ring.running:
            return original(*args, **kwargs)
        previous = sys.getprofile()
        owned = previous is None
        if threading.getprofile() is not None or (previous is not None and previous is not ring.callback):
            ring.uncertain = True
            return original(*args, **kwargs)
        ring.callback_active += 1
        if owned:
            sys.setprofile(ring.callback)
        try:
            return original(*args, **kwargs)
        finally:
            if owned:
                if sys.getprofile() is ring.callback:
                    sys.setprofile(None)
                else:
                    ring.uncertain = True
            ring.callback_active -= 1
    return observed


def callback_proxy(cache_class, ring):
    """Install before adoption; never replace the original product body."""
    original = cache_class._shrink
    observed = callback_attachment(ring, original)
    cache_class._shrink = observed
    def restore():
        if cache_class._shrink is observed:
            cache_class._shrink = original
        else:
            ring.uncertain = True
    return restore
