"""Real placed Qwen generation, live KV protection and native idle-owner retirement."""
from __future__ import annotations

import gc
import hashlib
import importlib.metadata
import json
import os
import sys
import tempfile
import time
from pathlib import Path

import msgspec
import torch
from cozy_runtime import lm
from cozy_runtime.author import App, Context
from cozy_runtime.author._model import PLACED
from tensorfs.varena import _runtime as varena
from tensorfs.varena import torch as vt

from .model import Loaded

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    gpu_uuid: str


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def settle():
    gc.collect()
    torch.cuda.synchronize()
    vt.trim(0)
    torch.cuda.synchronize()
    vt.trim(0)


def until(ctx, predicate, message):
    deadline = time.monotonic() + 10
    while True:
        ctx.raise_if_cancelled()
        if found := predicate():
            return found
        assert time.monotonic() < deadline, message
        time.sleep(.001)


def actual(payload, ctx):
    import cozy_runtime
    from cozy_runtime.internal import int8_linear
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext

    assert importlib.metadata.version('cozy-runtime') == payload.runtime_version
    assert importlib.metadata.version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    root = Path(cozy_runtime.__file__).resolve().parent
    expected = {'lm/__init__.py', 'lm/_kernels.py', 'author/_loader.py', 'author/_model.py',
                'internal/residency.py', 'internal/cache_owners.py', 'internal/weights.py',
                'internal/fill.py', 'internal/resolution.py', 'internal/encoding/selection.py',
                'graphs/__init__.py', 'graphs/domain.py', 'internal/int8_linear.py',
                'internal/kernel_ready.py', 'internal/prebuilt.py'}
    assert set(payload.runtime_modules) == expected
    for name, value in payload.runtime_modules.items():
        assert digest(root / name) == value, name
    assert torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    assert varena.CacheOwner.capability() == 1 and not lm.ABSENT
    assert importlib.metadata.version('transformers') == '5.17.0'
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert not request.memory.models
    assert int8_linear.native(torch, 0) is not None
    settle()
    return request, {'pid': os.getpid(), 'request_id': ctx.request_id,
                     'input': msgspec.to_builtins(payload), 'gpu_uuid': uuid,
                     'transformers': importlib.metadata.version('transformers')}


def ask(memory):
    stats = vt.stats(0)
    total = stats['mapped_bytes'] + stats['cache_mapped_bytes'] + memory.native.stats()['resident_bytes']
    request = varena.request_cache_shrink(0, max(1, total))
    assert request is not None
    return request


def event_context(domain, engine=None):
    """Read existing ownership metadata only; do not close/retire the live domain."""
    return {
        'domain_closed': domain.closed, 'domain_primed': domain.primed,
        'domain_fenced': domain.fenced, 'domain_graphs': len(domain.graphs),
        'library_stream': None if domain.stream is None else domain.stream.cuda_stream,
        'control_stream': None if domain.control is None else domain.control.cuda_stream,
        'last_stream': None if domain.last_stream is None else domain.last_stream.cuda_stream,
        'owner_pool_tag': getattr(domain.owner, '_varena_owner', None),
        'engine_graphs': None if engine is None else len(engine.graphs),
        'engine_addresses': None if engine is None else len(engine.addresses),
        'engine_where': None if engine is None else len(engine.where),
        'engine_cache_present': None if engine is None else engine.cache is not None,
        'engine_driver_present': None if engine is None else engine.driver is not None,
        'engine_pending': None if engine is None else bool(engine.pending),
        'attribution_limit': 'aggregate native counters do not expose block-versus-pool event census',
    }


@app.entrypoint(accelerator=True)
def lm_owner(payload: Probe, ctx: Context) -> Report:
    request, provenance = actual(payload, ctx)
    loaded = public_output = rebuilt = old_kv = ids = None
    result = {'passed': False, 'case': 'actual_placed_qwen_lm_owner', 'provenance': provenance,
              'scope': 'random-weight small real Qwen mechanism proof; no pretrained-model quality claim'}
    try:
        with tempfile.TemporaryDirectory(prefix='varena-lm-owner-') as directory:
            loaded = Loaded(request.memory, Path(directory))
            body = loaded.model.pipeline.body
            assert PLACED[body][0] is loaded.placement and not lm._unsupported(body)
            assert not lm._capture_contract(body)
            ids = torch.tensor([[1, 4, 9]], dtype=torch.long, device=request.memory.device)
            expected = loaded.expected(ids)
            # Ordinary process-owned library workspaces precede the lifecycle baseline.
            # The placed model and prompt remain alive on both sides of this comparison.
            domain = loaded.placement.graph_domain('body')
            domain.initialize()
            domain.prime_blas()
            settle()
            baseline = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['baseline'] = baseline
            result['event_context_baseline'] = event_context(domain)
            result['baseline_scope'] = 'placed module derived buffers, input IDs and shared library domain remain live'
            active = {}

            def progress(tokens):
                if active:
                    return
                try:
                    engine = lm._ENGINES[body]
                    region = engine.region
                    owner = region._cache_owner.native
                    before = owner.facts()
                    assert engine.driver is not None and engine._occupied() > 0 and region.pins > 0
                    queued = ask(request.memory)
                    until(ctx, lambda: owner.facts()['declined'] > before['declined'],
                          'live LM owner did not decline native shrink')
                    active.update(passed=True, tokens=tokens, occupied=engine._occupied(),
                                  region_pins=region.pins, owner=owner.facts(), request=queued.facts())
                except Exception as error:  # noqa: BLE001 -- public progress reports cannot abort decode
                    active.update(passed=False, error=f'{type(error).__name__}: {error}')

            public_output = loaded.model.decode(ids, progress=progress)
            result['active'] = active
            assert active.get('passed'), active
            assert torch.equal(public_output.cpu(), expected)
            engine = lm._ENGINES[body]
            loaded.placement.release()  # the ordinary end-of-attempt holds, before idle reclaim
            assert engine.recorded > 0 and engine.replayed > 0, engine.unrecorded
            assert engine.driver is None and engine._occupied() == 0 and not engine.pending
            assert engine.region.pins == 0 and engine.cache is not None
            owner = engine.region._cache_owner.native
            bindings = tuple(engine.addresses.values())
            assert bindings and all(binding.held for binding in bindings)
            old_kv = engine.cache.layers[0].keys[:, :, :ids.shape[1], :]
            old_kv_cpu = old_kv.cpu().clone()
            old_pointer = old_kv.data_ptr()
            before_idle = {'owner': owner.facts(), 'graphs': len(engine.graphs),
                           'recorded': engine.recorded, 'replayed': engine.replayed,
                           'retained': [sum(block.retained for block in b.groups) for b in bindings],
                           'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['before_idle'] = before_idle
            queued = ask(request.memory)
            until(ctx, lambda: engine.cache is None and not owner.facts()['running'],
                  'idle LM owner did not retire')
            assert not engine.graphs and not engine.addresses and not engine.where
            assert owner.facts()['returned'] > before_idle['owner']['returned']
            assert owner.facts()['failed'] is None and engine.region.fresh
            assert all(binding.closed and not binding.held for binding in bindings)
            assert torch.equal(old_kv.cpu(), old_kv_cpu) and torch.equal(public_output.cpu(), expected)
            after_idle = {'owner': owner.facts(), 'request': queued.facts(),
                          'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['after_idle'] = after_idle
            result['event_context_after_idle'] = event_context(domain, engine)
            rebuilt = loaded.model.decode(ids)
            assert torch.equal(rebuilt.cpu(), expected) and torch.equal(public_output.cpu(), expected)
            assert engine.cache is not None and engine.cache.layers[0].keys.data_ptr() != old_pointer
            assert engine.recorded > before_idle['recorded'] and engine.replayed > before_idle['replayed']
            assert torch.equal(old_kv.cpu(), old_kv_cpu)
            fresh_region = engine.region
            lm.release(body)
            assert body not in lm._ENGINES and fresh_region.closed
            until(ctx, lambda: owner.facts()['retired'], 'closed LM owner did not retire')
            assert not owner.facts()['requestable']
            assert torch.equal(public_output.cpu(), expected) and torch.equal(rebuilt.cpu(), expected)
            assert torch.equal(old_kv.cpu(), old_kv_cpu)
            result.update(active=active, before_idle=before_idle, after_idle=after_idle,
                          token_ids=expected.tolist(), real_model=type(body).__name__,
                          checkpoint=loaded.source.id, old_kv_pointer=old_pointer,
                          escaped_tokens_and_kv_preserved=True, owner_after_close=owner.facts(),
                          public_output_device=str(public_output.device), escaped_kv_device=str(old_kv.device),
                          baseline=baseline, reference_device=str(request.memory.device))
            public_output = rebuilt = old_kv = None
            loaded.close()
            assert not request.memory.models
            bindings = ()
            settle()
            request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
            result['final'] = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['event_context_final'] = event_context(domain, engine)
            assert result['final']['weights']['pinned_bytes'] == 0
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'event_handles', 'cache_mapped_bytes', 'cache_reserved_bytes',
                          'cache_live_bytes', 'cache_pending_bytes', 'cache_ready_bytes', 'caches'):
                assert result['final']['working'][field] == baseline['working'][field], (
                    field, baseline, result['final'])
            result['passed'] = True
            loaded = None
    except AssertionError as error:
        # Preserve every original assertion condition and attach its already-read facts
        # to the structured request failure; executor stderr may not be relayed.
        raise AssertionError(f'{error}; event_diagnostic={json.dumps(result, sort_keys=True)}') from error
    finally:
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        if loaded is not None:
            lm.release(loaded.model.pipeline.body)
            loaded.close()
        public_output = rebuilt = old_kv = ids = None
    return Report(True, json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def lm_event_plateau(payload: Probe, ctx: Context) -> Report:
    """Diagnostic final cell: same live Domain, repeated real LM lifecycles, then finish."""
    request, provenance = actual(payload, ctx)
    loaded = output = rebuilt = escaped = ids = None
    result = {'passed': False, 'case': 'same_domain_repeated_lm_event_metadata',
              'provenance': provenance, 'cycles': [],
              'scope': 'diagnostic only; original lm_owner strict baseline remains unchanged',
              'executor_domain_closed_at_end': True}
    try:
        with tempfile.TemporaryDirectory(prefix='varena-lm-events-') as directory:
            loaded = Loaded(request.memory, Path(directory))
            body = loaded.model.pipeline.body
            ids = torch.tensor([[1, 4, 9]], dtype=torch.long, device=request.memory.device)
            expected = loaded.expected(ids)
            domain = loaded.placement.graph_domain('body')
            domain.initialize()
            domain.prime_blas()
            settle()
            result['baseline'] = {'working': vt.stats(0), 'weights': request.memory.native.stats(),
                                  'context': event_context(domain)}
            for index in range(6):
                output = loaded.model.decode(ids)
                assert torch.equal(output.cpu(), expected)
                engine = lm._ENGINES[body]
                loaded.placement.release()
                assert engine.driver is None and engine._occupied() == 0 and not engine.pending
                assert engine.recorded > 0 and engine.replayed > 0
                owner = engine.region._cache_owner.native
                before = owner.facts()
                escaped = engine.cache.layers[0].keys[:, :, :ids.shape[1], :]
                escaped_cpu = escaped.cpu().clone()
                queued = ask(request.memory)
                until(ctx, lambda: engine.cache is None and not owner.facts()['running'],
                      'idle repeated-cycle owner did not retire')
                assert owner.facts()['returned'] > before['returned']
                assert not engine.graphs and not engine.addresses and not engine.where
                assert torch.equal(escaped.cpu(), escaped_cpu)
                rebuilt = loaded.model.decode(ids)
                assert torch.equal(rebuilt.cpu(), expected) and torch.equal(output.cpu(), expected)
                assert torch.equal(escaped.cpu(), escaped_cpu)
                loaded.placement.release()
                lm.release(body)
                assert body not in lm._ENGINES
                until(ctx, lambda: owner.facts()['retired'], 'repeated-cycle owner did not close')
                assert not owner.facts()['requestable'] and owner.facts()['failed'] is None
                output = rebuilt = escaped = None
                settle()
                request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
                facts = {'cycle': index, 'working': vt.stats(0),
                         'weights': request.memory.native.stats(), 'context': event_context(domain, engine),
                         'owner': owner.facts(), 'request': queued.facts()}
                assert not facts['context']['domain_closed'] and not facts['context']['engine_cache_present']
                result['cycles'].append(facts)
            counts = [row['working']['event_handles'] for row in result['cycles']]
            result['event_counts'] = counts
            result['last_three_event_counts_equal'] = len(set(counts[-3:])) == 1
            byte_fields = ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                           'pools', 'pools_bytes', 'cache_mapped_bytes', 'cache_reserved_bytes',
                           'cache_live_bytes', 'cache_pending_bytes', 'cache_ready_bytes', 'caches')
            result['last_three_byte_states_equal'] = all(
                len({row['working'][key] for row in result['cycles'][-3:]}) == 1 for key in byte_fields)
            loaded.close()
            loaded = None
            assert not request.memory.models
            settle()
            request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
            result['before_domain_finish'] = {'working': vt.stats(0),
                                             'context': event_context(domain, engine)}
            # This closes the executor's shared graph domain. It is a named final-cell
            # diagnostic, not permission to retire the caller-owned request stream.
            domain.finish()
            settle()
            result['after_domain_finish'] = {'working': vt.stats(0),
                                            'context': event_context(domain, engine)}
            assert domain.closed and domain.owner is None and domain.stream is None and domain.control is None
            result['request_stream_retired'] = False
            result['attribution_limit'] = 'native aggregate stats; shared request stream remains owned by Runtime'
            result['passed'] = True
    except Exception as error:
        raise RuntimeError(f'{type(error).__name__}: {error}; lifecycle_diagnostic={json.dumps(result, sort_keys=True)}') from error
    finally:
        if loaded is not None:
            lm.release(loaded.model.pipeline.body)
            loaded.close()
        output = rebuilt = escaped = ids = None
    return Report(True, json.dumps(result, sort_keys=True))


def fixed_lm_warmup(loaded, request, ctx, ids, expected, domain):
    """Exactly three representative cycles; never repeat a warmup until it passes."""
    byte_fields = ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                   'pools', 'pools_bytes', 'cache_mapped_bytes', 'cache_reserved_bytes',
                   'cache_live_bytes', 'cache_pending_bytes', 'cache_ready_bytes', 'caches')
    settle()
    initial = vt.stats(0)
    snapshots = []
    body = loaded.model.pipeline.body
    for index in range(3):
        active = {}

        def progress(tokens):
            if active:
                return
            try:
                live = lm._ENGINES[body]
                live_owner = live.region._cache_owner.native
                before_live = live_owner.facts()
                assert live.driver is not None and live._occupied() > 0 and live.region.pins > 0
                shrink = ask(request.memory)
                until(ctx, lambda: live_owner.facts()['declined'] > before_live['declined'],
                      'warm active LM owner did not decline shrink')
                active.update(passed=True, tokens=tokens, owner=live_owner.facts(), request=shrink.facts())
            except Exception as error:
                active.update(passed=False, error=f'{type(error).__name__}: {error}')

        output = loaded.model.decode(ids, progress=progress)
        assert active.get('passed'), active
        assert torch.equal(output.cpu(), expected)
        engine = lm._ENGINES[body]
        loaded.placement.release()
        assert engine.driver is None and engine._occupied() == 0 and not engine.pending
        assert engine.recorded > 0 and engine.replayed > 0 and engine.region.pins == 0
        owner = engine.region._cache_owner.native
        before = owner.facts()
        bindings = tuple(engine.addresses.values())
        assert bindings and all(binding.held for binding in bindings)
        escaped = engine.cache.layers[0].keys[:, :, :ids.shape[1], :]
        escaped_cpu = escaped.cpu().clone()
        queued = ask(request.memory)
        until(ctx, lambda: engine.cache is None and not owner.facts()['running'],
              'warm idle LM owner did not retire')
        assert owner.facts()['returned'] > before['returned'] and owner.facts()['failed'] is None
        assert not engine.graphs and not engine.addresses and not engine.where
        assert all(binding.closed and not binding.held for binding in bindings)
        assert torch.equal(escaped.cpu(), escaped_cpu) and torch.equal(output.cpu(), expected)
        rebuilt = loaded.model.decode(ids)
        assert torch.equal(rebuilt.cpu(), expected) and torch.equal(output.cpu(), expected)
        assert torch.equal(escaped.cpu(), escaped_cpu)
        fresh_region = engine.region
        fresh_owner = fresh_region._cache_owner.native
        loaded.placement.release()
        lm.release(body)
        assert body not in lm._ENGINES and fresh_region.closed
        until(ctx, lambda: owner.facts()['retired'] and fresh_owner.facts()['retired'],
              'warm LM owners did not close')
        for closed in (owner, fresh_owner):
            assert not closed.facts()['requestable'] and closed.facts()['failed'] is None
        assert not engine.graphs and not engine.addresses and not engine.where and engine.cache is None
        output = rebuilt = escaped = None
        bindings = ()
        settle()
        request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
        working = vt.stats(0)
        weights = request.memory.native.stats()
        for field in byte_fields:
            assert working[field] == initial[field], ('warm byte cleanup', index, field, initial, working)
        assert working['allocations'] - working['frees'] == initial['allocations'] - initial['frees']
        for field in ('resident_bytes', 'pinned_bytes', 'ready_bytes', 'reclaimable_bytes'):
            assert weights[field] == 0, ('warm weight cleanup', index, field, weights)
        assert not weights['poisoned']
        snapshots.append({'cycle': index, 'active': active, 'working': working, 'weights': weights,
                          'owner': owner.facts(), 'fresh_owner': fresh_owner.facts(),
                          'context': event_context(domain, engine), 'request': queued.facts()})
    assert snapshots[-1]['working']['event_handles'] == snapshots[-2]['working']['event_handles'], (
        'fixed warmup event counts did not stabilize', snapshots)
    assert snapshots[-1]['weights']['regions'] == snapshots[-2]['weights']['regions']
    assert snapshots[-1]['weights']['reserved_bytes'] == snapshots[-2]['weights']['reserved_bytes']
    return {'fixed_cycles': 3, 'initial_working': initial, 'snapshots': snapshots,
            'last_two_event_counts_equal': True,
            'scope': 'same live model/Domain; physical weights and per-cycle working bytes cleaned; model-owned VA may remain'}


@app.entrypoint(accelerator=True)
def lm_owner_warmed(payload: Probe, ctx: Context) -> Report:
    request, provenance = actual(payload, ctx)
    loaded = public_output = rebuilt = old_kv = ids = None
    result = {'passed': False, 'case': 'warmed_shared_allocator_domain_lm_owner', 'provenance': provenance,
              'scope': 'warmed shared allocator/Domain lifecycle; no cold zero-event-growth or exact original18-event attribution claim'}
    try:
        with tempfile.TemporaryDirectory(prefix='varena-lm-owner-') as directory:
            loaded = Loaded(request.memory, Path(directory))
            body = loaded.model.pipeline.body
            assert PLACED[body][0] is loaded.placement and not lm._unsupported(body)
            assert not lm._capture_contract(body)
            ids = torch.tensor([[1, 4, 9]], dtype=torch.long, device=request.memory.device)
            expected = loaded.expected(ids)
            # Ordinary process-owned library workspaces precede the lifecycle baseline.
            # The placed model and prompt remain alive on both sides of this comparison.
            domain = loaded.placement.graph_domain('body')
            domain.initialize()
            domain.prime_blas()
            settle()
            result['warmup'] = fixed_lm_warmup(loaded, request, ctx, ids, expected, domain)
            settle()
            baseline = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['baseline'] = baseline
            result['event_context_baseline'] = event_context(domain)
            result['baseline_scope'] = 'placed module derived buffers, input IDs and shared library domain remain live'
            active = {}

            def progress(tokens):
                if active:
                    return
                try:
                    engine = lm._ENGINES[body]
                    region = engine.region
                    owner = region._cache_owner.native
                    before = owner.facts()
                    assert engine.driver is not None and engine._occupied() > 0 and region.pins > 0
                    queued = ask(request.memory)
                    until(ctx, lambda: owner.facts()['declined'] > before['declined'],
                          'live LM owner did not decline native shrink')
                    active.update(passed=True, tokens=tokens, occupied=engine._occupied(),
                                  region_pins=region.pins, owner=owner.facts(), request=queued.facts())
                except Exception as error:  # noqa: BLE001 -- public progress reports cannot abort decode
                    active.update(passed=False, error=f'{type(error).__name__}: {error}')

            public_output = loaded.model.decode(ids, progress=progress)
            result['active'] = active
            assert active.get('passed'), active
            assert torch.equal(public_output.cpu(), expected)
            engine = lm._ENGINES[body]
            loaded.placement.release()  # the ordinary end-of-attempt holds, before idle reclaim
            assert engine.recorded > 0 and engine.replayed > 0, engine.unrecorded
            assert engine.driver is None and engine._occupied() == 0 and not engine.pending
            assert engine.region.pins == 0 and engine.cache is not None
            owner = engine.region._cache_owner.native
            bindings = tuple(engine.addresses.values())
            assert bindings and all(binding.held for binding in bindings)
            old_kv = engine.cache.layers[0].keys[:, :, :ids.shape[1], :]
            old_kv_cpu = old_kv.cpu().clone()
            old_pointer = old_kv.data_ptr()
            before_idle = {'owner': owner.facts(), 'graphs': len(engine.graphs),
                           'recorded': engine.recorded, 'replayed': engine.replayed,
                           'retained': [sum(block.retained for block in b.groups) for b in bindings],
                           'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['before_idle'] = before_idle
            queued = ask(request.memory)
            until(ctx, lambda: engine.cache is None and not owner.facts()['running'],
                  'idle LM owner did not retire')
            assert not engine.graphs and not engine.addresses and not engine.where
            assert owner.facts()['returned'] > before_idle['owner']['returned']
            assert owner.facts()['failed'] is None and engine.region.fresh
            assert all(binding.closed and not binding.held for binding in bindings)
            assert torch.equal(old_kv.cpu(), old_kv_cpu) and torch.equal(public_output.cpu(), expected)
            after_idle = {'owner': owner.facts(), 'request': queued.facts(),
                          'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['after_idle'] = after_idle
            result['event_context_after_idle'] = event_context(domain, engine)
            rebuilt = loaded.model.decode(ids)
            assert torch.equal(rebuilt.cpu(), expected) and torch.equal(public_output.cpu(), expected)
            assert engine.cache is not None and engine.cache.layers[0].keys.data_ptr() != old_pointer
            assert engine.recorded > before_idle['recorded'] and engine.replayed > before_idle['replayed']
            assert torch.equal(old_kv.cpu(), old_kv_cpu)
            fresh_region = engine.region
            lm.release(body)
            assert body not in lm._ENGINES and fresh_region.closed
            until(ctx, lambda: owner.facts()['retired'], 'closed LM owner did not retire')
            assert not owner.facts()['requestable']
            assert torch.equal(public_output.cpu(), expected) and torch.equal(rebuilt.cpu(), expected)
            assert torch.equal(old_kv.cpu(), old_kv_cpu)
            result.update(active=active, before_idle=before_idle, after_idle=after_idle,
                          token_ids=expected.tolist(), real_model=type(body).__name__,
                          checkpoint=loaded.source.id, old_kv_pointer=old_pointer,
                          escaped_tokens_and_kv_preserved=True, owner_after_close=owner.facts(),
                          public_output_device=str(public_output.device), escaped_kv_device=str(old_kv.device),
                          baseline=baseline, reference_device=str(request.memory.device))
            public_output = rebuilt = old_kv = None
            loaded.close()
            assert not request.memory.models
            bindings = ()
            settle()
            request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
            result['final'] = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
            result['event_context_final'] = event_context(domain, engine)
            assert result['final']['weights']['pinned_bytes'] == 0
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'event_handles', 'cache_mapped_bytes', 'cache_reserved_bytes',
                          'cache_live_bytes', 'cache_pending_bytes', 'cache_ready_bytes', 'caches'):
                assert result['final']['working'][field] == baseline['working'][field], (
                    field, baseline, result['final'])
            result['passed'] = True
            loaded = None
    except AssertionError as error:
        # Preserve every original assertion condition and attach its already-read facts
        # to the structured request failure; executor stderr may not be relayed.
        raise AssertionError(f'{error}; event_diagnostic={json.dumps(result, sort_keys=True)}') from error
    finally:
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        if loaded is not None:
            lm.release(loaded.model.pipeline.body)
            loaded.close()
        public_output = rebuilt = old_kv = ids = None
    return Report(True, json.dumps(result, sort_keys=True))
