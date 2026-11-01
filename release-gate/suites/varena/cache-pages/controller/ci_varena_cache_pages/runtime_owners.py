"""Real Runtime graph owners, allocator pressure and the same-GIL refusal boundary."""
from __future__ import annotations

import ctypes
import gc
import json
import threading
import time
import weakref
from pathlib import Path

import torch
from tensorfs import varena
from tensorfs.varena import torch as vt

from . import Report, digest, setup

MIB = 1 << 20


def actual(payload, ctx):
    import cozy_runtime
    from cozy_runtime.author import _loop as declared_loop
    from cozy_runtime.internal import loop
    from cozy_runtime.internal.residency import _REQUEST, Request

    _, provenance = setup(payload, ctx, cache_required=True)
    assert varena.CacheOwner.capability() == 1
    assert provenance['versions']['cozy-runtime'] == payload.runtime_version
    expected = {'internal/cache_owners.py', 'internal/loop.py', 'internal/prepared.py',
                'internal/residency.py', 'internal/weights.py', 'graphs/__init__.py',
                'graphs/domain.py', 'lm/__init__.py'}
    assert set(payload.runtime_modules) == expected
    root = Path(cozy_runtime.__file__).resolve().parent
    for name, value in payload.runtime_modules.items():
        assert digest(root / name) == value, name
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and not request.memory.models
    assert isinstance(declared_loop._RUNTIME, loop.Loops)
    return request, declared_loop._RUNTIME, provenance


def settle():
    gc.collect()
    torch.cuda.synchronize()
    vt.trim(0)
    torch.cuda.synchronize()
    vt.trim(0)


def until(ctx, predicate, description, seconds=10):
    deadline = time.monotonic() + seconds
    while True:
        ctx.raise_if_cancelled()
        found = predicate()
        if found:
            return found
        assert time.monotonic() < deadline, description
        time.sleep(.001)


def raw_cache_value(cache, value, *, write=True):
    driver = ctypes.CDLL('libcuda.so.1')
    fill = driver.cuMemsetD8_v2
    fill.argtypes, fill.restype = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t], ctypes.c_int
    read = driver.cuMemcpyDtoH_v2
    read.argtypes, read.restype = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t], ctypes.c_int
    if write:
        assert fill(cache.base, value, cache.reserved_bytes) == 0
    for offset in range(0, cache.reserved_bytes, 8 * MIB):
        count = min(8 * MIB, cache.reserved_bytes - offset)
        block = (ctypes.c_ubyte * count)()
        assert read(block, cache.base + offset, count) == 0
        assert bytes(block) == bytes([value]) * count


def full_tensor_value(tensor, value):
    for first in range(0, tensor.numel(), 8 * MIB):
        actual = tensor[first:first + 8 * MIB].cpu()
        assert bool((actual == value).all())


def allocator_progress(payload, ctx):
    request, _, provenance = actual(payload, ctx)
    assert request.memory.cooperation is not None and request.memory.cooperation.facts()['active']
    cache = varena.Cache(0, 256 * MIB)
    cache.commit(0, cache.reserved_bytes)
    raw_cache_value(cache, 97)
    calls = []

    def shrink(bytes):
        calls.append({'wanted': bytes, 'at_ns': time.monotonic_ns()})
        cache.release(0, cache.reserved_bytes, 0)

    owner = varena.CacheOwner(0, shrink)
    filler = output = None
    try:
        settle()
        free, total = torch.cuda.mem_get_info(0)
        margin = 128 * MIB
        filler_bytes = (free - margin) // (2 * MIB) * (2 * MIB)
        assert filler_bytes > 0
        filler = torch.empty(filler_bytes, dtype=torch.uint8, device=ctx.device)
        filler.fill_(53)
        full_tensor_value(filler, 53)
        available, _ = torch.cuda.mem_get_info(0)
        assert available < 192 * MIB
        before = {'working': vt.stats(0), 'owner': owner.facts(),
                  'cooperation': request.memory.cooperation.facts(), 'cache': cache.stats()}
        began = time.monotonic_ns()
        output = torch.empty(192 * MIB, dtype=torch.uint8, device=ctx.device)
        output.fill_(71)
        full_tensor_value(output, 71)
        ended = time.monotonic_ns()
        after = {'working': vt.stats(0), 'owner': owner.facts(),
                 'cooperation': request.memory.cooperation.facts(), 'cache': cache.stats()}
        assert after['working']['driver_physical_ooms'] > before['working']['driver_physical_ooms']
        assert after['cache']['mapped_bytes'] == 0 and len(calls) == 1
        assert began <= calls[0]['at_ns'] <= ended
        recovery = after['cooperation']['last_recovery']
        assert recovery['outcome'] == 'local_physical_release_observed', recovery
        assert recovery['released_bytes'] == recovery['completed_bytes'] == 0
        owner.close()
        cache.close()
        del output, filler
        output = filler = None
        settle()
        return Report(True, json.dumps({'passed': True, 'case': 'allocator_owner_progress',
                                       'provenance': provenance, 'filler_bytes': filler_bytes,
                                       'observed_free': available, 'driver_total': total,
                                       'requested_output': 192 * MIB, 'full_readback': True,
                                       'calls': calls, 'before': before, 'after': after,
                                       'final': vt.stats(0)}, sort_keys=True))
    finally:
        owner.defer_close()
        del output, filler
        # Do not fabricate release on failure: native Cache retains any unproven live pages.


class ResizeResult(ctypes.Structure):
    _fields_ = [('gil_before', ctypes.c_int), ('gil_after', ctypes.c_int),
                ('failed', ctypes.c_int), ('callback_marker', ctypes.c_int),
                ('error', ctypes.c_char * 2048)]


def held_gil(payload, ctx):
    request, _, provenance = actual(payload, ctx)
    path = Path(__file__).parent / 'bin/gil_resize.so'
    assert digest(path) == payload.gil_gate_sha256
    gate = ctypes.PyDLL(str(path))
    resize = gate.resize_holding_gil
    resize.argtypes = [ctypes.py_object, ctypes.c_uint64, ctypes.POINTER(ctypes.c_int),
                       ctypes.POINTER(ResizeResult)]
    resize.restype = ctypes.c_int
    marker = ctypes.c_int(0)
    cache = varena.Cache(0, 16 * MIB)
    cache.commit(0, cache.reserved_bytes)
    raw_cache_value(cache, 83)
    storage = torch.UntypedStorage(2 * MIB, device=ctx.device)
    view = torch.empty(0, dtype=torch.uint8, device=ctx.device).set_(storage, 0, (2 * MIB,), (1,))
    view.fill_(7)
    settle()

    def shrink(bytes):
        assert bytes > 0
        marker.value += 1
        cache.release(0, cache.reserved_bytes, 0)

    owner = varena.CacheOwner(0, shrink)
    before = vt.stats(0)
    limit = before['mapped_bytes'] + before['cache_mapped_bytes'] + request.memory.native.stats()['resident_bytes']
    vt.set_limit(0, limit)
    try:
        first = ResizeResult()
        assert resize(storage, 16 * MIB, ctypes.byref(marker), ctypes.byref(first)) == 0
        assert first.gil_before == first.gil_after == 1
        assert first.failed and first.callback_marker == 0, (first.failed, first.callback_marker)
        assert 'memory' in bytes(first.error).decode().lower(), bytes(first.error)
        assert storage.nbytes() == 2 * MIB
        # This wait deliberately releases the GIL only AFTER recording the original refusal.
        until(ctx, lambda: cache.stats()['mapped_bytes'] == 0, 'Python owner did not complete after GIL release')
        assert marker.value == 1 and owner.facts()['returned'] == 1
        second = ResizeResult()
        assert resize(storage, 16 * MIB, ctypes.byref(marker), ctypes.byref(second)) == 0
        assert second.gil_before == second.gil_after == 1 and not second.failed
        assert storage.nbytes() == 16 * MIB
        full_tensor_value(view, 7)
        after = vt.stats(0)
        assert after['driver_physical_ooms'] == before['driver_physical_ooms']
        facts = owner.facts()
        owner.close()
        cache.close()
        return Report(True, json.dumps({'passed': True, 'case': 'same_gil_owner_refusal',
                                       'provenance': provenance, 'managed_limit': limit,
                                       'first': {'gil_before': first.gil_before, 'gil_after': first.gil_after,
                                                 'failed': bool(first.failed), 'callback_marker': first.callback_marker,
                                                 'error': bytes(first.error).decode()},
                                       'later_allocation': {'gil_held': second.gil_after, 'bytes': storage.nbytes(),
                                                            'preserved_old_bytes': 2 * MIB},
                                       'owner': facts, 'before': before, 'after': after,
                                       'boundary': 'initial actual allocation exhausted while holding the GIL; later physical release and later allocation are separate'}, sort_keys=True))
    finally:
        vt.set_limit(0, None)
        owner.defer_close()


def capture_close(payload, ctx):
    _, _, provenance = actual(payload, ctx)
    settle()
    baseline = vt.stats(0)
    cache = varena.Cache(0, 2 * MIB)
    cache.commit(0, cache.reserved_bytes)
    raw_cache_value(cache, 109)
    calls = []

    def shrink(wanted):
        calls.append(wanted)
        cache.release(0, cache.reserved_bytes, 0)

    owner = varena.CacheOwner(0, shrink)
    graph = torch.cuda.CUDAGraph()
    value = torch.zeros(1, dtype=torch.uint8, device=ctx.device)
    value.add_(1)  # warm the actual in-place kernel before capture
    torch.cuda.synchronize()
    try:
        with vt.graph(graph, reserve_bytes=0):
            value.add_(1)
            before = vt.stats(0)
            queued = varena.request_cache_shrink(0, cache.reserved_bytes)
            assert queued is not None
            until(ctx, lambda: owner.facts()['deferred'] > 0,
                  'actual capture did not defer the queued owner')
            failure = None
            try:
                owner.close()
            except varena.CacheOwnerDeferredError as error:
                failure = {'type': type(error).__name__, 'reason': error.reason, 'message': str(error)}
            during = owner.facts()
            after = vt.stats(0)
            assert failure is not None and failure['reason'] == 'capture'
            assert during['closing'] and not during['retired'] and not during['requestable']
            assert not calls
            assert before['driver_memory_calls'] == after['driver_memory_calls']
            assert before['cache_live_bytes'] == after['cache_live_bytes'] == cache.reserved_bytes
        until(ctx, lambda: owner.facts()['retired'], 'owner did not retire after actual capture ended')
        owner.close()
        # Retiring a weak callback cannot invent a release contract for its live Cache.
        assert not calls and cache.stats()['live_bytes'] == cache.reserved_bytes
        raw_cache_value(cache, 109, write=False)
        vt.replay(graph)
        torch.cuda.synchronize()
        full_tensor_value(value, 2)
        retired = owner.facts()
        graph.reset()
        graph = value = None
        cache.release(0, cache.reserved_bytes, 0)
        torch.cuda.synchronize()
        cache.poll()
        cache.close()
        settle()
        final = vt.stats(0)
        for name in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools_bytes',
                     'cache_mapped_bytes', 'cache_reserved_bytes', 'caches'):
            assert final[name] == baseline[name], (name, baseline, final)
        return Report(True, json.dumps({'passed': True, 'case': 'python_owner_capture_close',
                                       'provenance': provenance, 'refusal': failure,
                                       'during': during, 'retired': retired, 'callback_calls': calls,
                                       'before': before, 'after': after, 'baseline': baseline,
                                       'final': final, 'live_cache_full_readback': True,
                                       'boundary': 'product working-driver calls only; fixture graph capture/replay calls are outside this counter'}, sort_keys=True))
    finally:
        owner.defer_close()
        if graph is not None:
            graph.reset()


def prepared_sequence(ctx, *, stages, captured_state=False, sleep=0):
    from cozy_runtime._kernels import _execution
    from cozy_runtime.author._model import LANDED
    from cozy_runtime.internal.prepared import PreparedRepeat, PreparedSequence

    pool, stream = torch.cuda.graph_pool_handle(), torch.cuda.Stream(device=ctx.device)
    state = None if captured_state else torch.zeros(4 * MIB, dtype=torch.uint8, device=ctx.device)
    definitions, footprints = [], []
    for _ in range(stages):
        index = torch.zeros((), dtype=torch.int64, device=ctx.device)
        graph = torch.cuda.CUDAGraph(keep_graph=True)
        footprint = vt.capture_footprint(0, pool)
        with vt.graph(graph, pool=pool, stream=stream, reserve_bytes=24 * MIB, footprint=footprint):
            if state is None:
                state = torch.empty(4 * MIB, dtype=torch.uint8, device=ctx.device)
            if captured_state:
                state.fill_(7)
            else:
                state.add_(1)
            if sleep:
                torch.cuda._sleep(sleep)
        native = _execution.ResidentRepeat(graph.raw_cuda_graph(), index.data_ptr(), 0, 1,
                                           [graph, state, index, stream, footprint])
        dependencies = tuple((weakref.ref(value), PreparedRepeat._layout(value)) for value in (state, index))
        definition = PreparedRepeat(native, (state,), 0, 1, dependencies, (), LANDED[0])
        definition._footprints = (footprint,)
        definitions.append(definition)
        footprints.append(footprint)
    return PreparedSequence(definitions, outputs=(state,), prefetch=False), tuple(footprints)


def graph_retirement(payload, ctx):
    from cozy_runtime.internal import loop

    request, loops, provenance = actual(payload, ctx)
    warm, _ = prepared_sequence(ctx, stages=1)
    warm.submit(ctx).wait()
    warm.close()
    del warm
    settle()
    baseline = vt.stats(0)
    a, a_footprints = prepared_sequence(ctx, stages=3, sleep=1_000_000_000)
    b, b_footprints = prepared_sequence(ctx, stages=1, captured_state=True, sleep=1_000_000_000)
    program = loop._Program((), b, 'resident', tuple(b.state), None,
                            ('cache-owner-fixture', 1), 1, 0)
    site = ('cache-owner-fixture', id(program))
    loops._install(site, program, request.memory)
    assert program.region._cache_owner is not None
    native_owner = program.region._cache_owner.native
    first_execution = a_execution = None
    release = threading.Event()
    retired = threading.Event()
    alias = None
    try:
        # An actual target lease must decline while its NativeSequence is active.
        with program.lock, program.region.pin():
            first_execution = b.submit(ctx)
            original = native_owner.facts()
            active_request = varena.request_cache_shrink(0, 24 * MIB)
            assert active_request is not None
            until(ctx, lambda: native_owner.facts()['declined'] > original['declined'],
                  'active target owner did not decline')
            assert not program.closed and not b._closed
            full_tensor_value(first_execution.wait()[0], 7)
        first_execution = None
        alias = b.state[0][::2]  # genuine live captured-output storage, outside the owner
        pool_before = vt.stats(0)['pools_bytes']
        original_shrink = program.region.shrink

        def close_then_hold(shrink=original_shrink):
            assert shrink is not None
            if not shrink():
                return False
            retired.set()
            # Real owner bookkeeping after real graph closure. Submission/domain locks
            # have been released; the target Region still blocks new target pins. Another
            # sequence must advance while native callback capture exclusion remains entered.
            assert release.wait(10), 'fixture release protocol deadline'
            return True

        program.region.shrink = close_then_hold
        del b  # the Program is now the only definition owner; the alias retains storage only
        a.state[0].zero_()
        a_execution = a.submit(ctx)
        until(ctx, lambda execution=a_execution: (p := execution.poll()).completed_steps >= 1 and not p.done,
              'no actual intermediate NativeSequence stage observed')
        request_handle = varena.request_cache_shrink(0, 24 * MIB)
        assert request_handle is not None
        until(ctx, retired.is_set, 'idle graph owner did not retire')
        during = until(ctx, lambda execution=a_execution: (p if (p := execution.poll()).completed_steps >= 2 and not p.done else None),
                       'other sequence did not enter later native stages during retirement')
        assert native_owner.facts()['running'] and program.closed
        assert all(trace.facts()['retired'] for trace in b_footprints)
        vt.trim(0)
        with_alias = vt.stats(0)
        assert with_alias['pools_bytes'] == pool_before, 'escaped pool storage was reclaimed early'
        release.set()
        until(ctx, lambda: not native_owner.facts()['running'], 'owner callback did not finish')
        full_tensor_value(a_execution.wait()[0], 3)
        full_tensor_value(alias, 7)
        # Reclaim is now eligible with no unrelated replay. The escaped storage itself,
        # rather than A's replay guard, must continue to retain the retired pool.
        vt.trim(0)
        idle_with_alias = vt.stats(0)
        assert idle_with_alias['pools_bytes'] == pool_before
        alias = None
        gc.collect()
        vt.trim(0)
        after_alias = vt.stats(0)
        assert after_alias['pools_bytes'] < idle_with_alias['pools_bytes'], 'last escaped alias did not release its pool'
        result = {'passed': True, 'case': 'actual_native_sequence_and_idle_graph_retirement',
                  'provenance': provenance, 'active_target_declined': True,
                  'other_sequence_completed_while_callback_running': during.completed_steps,
                  'other_sequence_result': 3, 'escaped_output_result': 7,
                  'owner': native_owner.facts(), 'with_alias': with_alias,
                  'idle_with_alias': idle_with_alias, 'after_alias': after_alias}
    finally:
        release.set()
        if first_execution is not None:
            first_execution.wait()
        if a_execution is not None:
            a_execution.wait()
        a.close()
        with loops._lock:
            loops._kept.pop(site, None)
        loop._close_idle(program)
        alias = None
    del a, a_execution, program, original_shrink, close_then_hold
    settle()
    final = vt.stats(0)
    result.update(baseline=baseline, final=final, footprints=[trace.facts() for trace in a_footprints])
    for name in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes', 'pools', 'pools_bytes'):
        assert final[name] == baseline[name], (name, baseline, final)
    return Report(True, json.dumps(result, sort_keys=True))
