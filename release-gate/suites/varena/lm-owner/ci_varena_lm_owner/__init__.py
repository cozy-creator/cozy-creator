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
from tensorfs import varena
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
            assert result['final']['weights']['pinned_bytes'] == 0
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'event_handles', 'cache_mapped_bytes', 'cache_reserved_bytes',
                          'cache_live_bytes', 'cache_pending_bytes', 'cache_ready_bytes', 'caches'):
                assert result['final']['working'][field] == baseline['working'][field], (
                    field, baseline, result['final'])
            result['passed'] = True
            loaded = None
    finally:
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        if loaded is not None:
            lm.release(loaded.model.pipeline.body)
            loaded.close()
        public_output = rebuilt = old_kv = ids = None
    return Report(True, json.dumps(result, sort_keys=True))
