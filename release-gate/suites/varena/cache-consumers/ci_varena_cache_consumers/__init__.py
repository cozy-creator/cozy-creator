"""Actual model-owned cache lifetime through ordinary Cozy execution."""
from __future__ import annotations

import gc
import hashlib
import importlib.metadata
import json
import os
import tempfile
import time
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context
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
        if value := predicate():
            return value
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
    expected = {'author/_loader.py', 'author/_model.py', 'internal/residency.py',
                'internal/cache_owners.py', 'internal/weights.py', 'internal/fill.py',
                'internal/resolution.py', 'internal/encoding/selection.py',
                'internal/int8_linear.py', 'internal/kernel_ready.py', 'internal/prebuilt.py'}
    assert set(payload.runtime_modules) == expected
    for name, value in payload.runtime_modules.items():
        assert digest(root / name) == value, name
    assert torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    assert varena.CacheOwner.capability() == 1
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert not request.memory.models
    assert int8_linear.native(torch, 0) is not None
    settle()
    return request, {'pid': os.getpid(), 'request_id': ctx.request_id,
                     'input': msgspec.to_builtins(payload), 'gpu_uuid': uuid}


def ask(memory):
    working = vt.stats(0)
    # A real dispose intent over the process's current managed bytes. It grants nothing;
    # the live baseline prevents an unrelated tiny ready page from finishing this test ask.
    wanted = working['mapped_bytes'] + working['cache_mapped_bytes'] + memory.native.stats()['resident_bytes']
    request = varena.request_cache_shrink(0, max(1, wanted))
    assert request is not None
    return request


def full_value(tensor):
    for at in range(0, tensor.numel(), 1 << 20):
        assert bool((tensor[at:at + (1 << 20)].cpu() == .25).all())


@app.entrypoint(accelerator=True)
def derived_cache(payload: Probe, ctx: Context) -> Report:
    request, provenance = actual(payload, ctx)
    initial = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
    loaded = escaped = rebuilt = output = None
    result = {'passed': False, 'case': 'actual_derived_cache_nested_scopes', 'provenance': provenance,
              'initial': initial}
    try:
        with tempfile.TemporaryDirectory(prefix='varena-derived-cache-') as directory, torch.no_grad():
            loaded = Loaded(request.memory, Path(directory))
            warm = loaded.model.warm()
            assert torch.equal(warm.cpu(), torch.full((1, 4), 2.))
            del warm
            settle()
            baseline = vt.stats(0)
            assert loaded.model.derived._region is None

            def active(cache):
                region = cache._region
                owner = region._cache_owner.native
                before = owner.facts()
                queued = ask(request.memory)
                until(ctx, lambda: owner.facts()['declined'] > before['declined'], 'active derived owner did not decline')
                facts = owner.facts()
                assert region.pins == 1 and cache.entries == 1 and not facts['failed']
                return {'region_pins': region.pins, 'owner': facts, 'request': queued.facts()}

            escaped, output, active_proof, reused = loaded.model.outer(active)
            assert reused and torch.equal(output.cpu(), torch.full((1, 4), 1.25))
            full_value(escaped)
            output = None
            settle()
            region = loaded.model.derived._region
            native_owner = region._cache_owner.native
            assert region.pins == 0
            before_idle = {'working': vt.stats(0), 'owner': native_owner.facts()}
            queued = ask(request.memory)
            until(ctx, lambda: loaded.model.derived.entries == 0, 'idle derived cache did not discard')
            until(ctx, lambda: not native_owner.facts()['running'], 'idle derived action did not finish')
            assert native_owner.facts()['returned'] > before_idle['owner']['returned']
            full_value(escaped)
            after_idle = {'working': vt.stats(0), 'owner': native_owner.facts(), 'request': queued.facts()}
            assert after_idle['working']['allocated_bytes'] == before_idle['working']['allocated_bytes']
            old_pointer = escaped.data_ptr()
            rebuilt, output, _, reused = loaded.model.outer(lambda _: None)
            assert reused and rebuilt.data_ptr() != old_pointer
            assert torch.equal(output.cpu(), torch.full((1, 4), 1.25))
            full_value(escaped)
            full_value(rebuilt)
            output = escaped = None
            settle()
            # Hold the actual model and a real output alias after ordinary registry unload.
            model = loaded.model
            loaded.close()
            loaded = None
            assert not model._cozy_ready and model.derived._region is None and region.closed
            closed = native_owner.facts()
            assert closed['closing'] and closed['retired'] and not closed['requestable']
            full_value(rebuilt)
            result.update(active=active_proof, before_idle=before_idle, after_idle=after_idle,
                          owner_after_unload=closed, retained_model_closed=True,
                          old_pointer=old_pointer, escaped_full_readback=True,
                          rebuilt_full_readback_after_unload=True, baseline=baseline)
            rebuilt = model = None
            settle()
            request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
            final = {'working': vt.stats(0), 'weights': request.memory.native.stats()}
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'cache_mapped_bytes', 'cache_reserved_bytes', 'caches'):
                assert final['working'][field] == baseline[field], (field, baseline, final)
            assert final['weights']['resident_bytes'] == initial['weights']['resident_bytes']
            result.update(passed=True, final=final)
    finally:
        if loaded is not None:
            loaded.close()
        escaped = rebuilt = output = None
    return Report(True, json.dumps(result, sort_keys=True))
