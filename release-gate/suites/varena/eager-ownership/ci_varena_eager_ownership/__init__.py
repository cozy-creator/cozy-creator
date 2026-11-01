"""Actual Request/Loop ownership; real CUDA owners and completion, never a fake Context."""
from __future__ import annotations

import contextvars
import gc
import hashlib
import json
import os
import tempfile
import threading
from contextlib import ExitStack
from importlib.metadata import version
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context, denoise_loop
from torch import nn

app = App()
MIB = 1 << 20


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    ownership_sha256: str
    loop_sha256: str
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


class Add(nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = nn.Parameter(torch.empty(MIB, device='meta', dtype=torch.float32),
                                   requires_grad=False)

    def forward(self, x):
        return x + self.weight


class StopStep(Exception):
    pass


def exercise(request, vt, device):
    # Importing LM registers a real decoder handler. No engine/cache is constructed by
    # import, just as H3's VLM imports LM without generating text on its denoising path.
    from cozy_runtime import lm
    from cozy_runtime.graphs import domain
    from cozy_runtime.internal import residency
    from cozy_runtime.internal.execution import ownership, phase

    memory = request.memory
    assert not lm._ENGINES
    marker = torch.zeros(1, device=memory.device, dtype=torch.int64)
    rows = []

    def invoke(name, body, *, expect_refusal=False):
        owned = ExitStack()
        seen = {}
        before = len(request.working_phases)

        def step(index, state):
            del index, state
            observation = phase._ACTIVE.get()
            assert observation is not None and observation.eager_commit is not None
            commit = observation.eager_commit
            assert commit.request is request and commit.observation is observation
            seen['observation'] = observation
            seen['commit'] = commit
            try:
                owned.enter_context(ownership.eager(commit))
            except ownership.OwnerConflict as error:
                assert expect_refusal, str(error)
                seen['refusal'] = str(error)
                return
            assert not expect_refusal, 'a real conflicting owner was admitted'
            seen.update(body(observation, commit))

        try:
            denoise_loop(step, range(1), state=(marker,), key=('eager-ownership', name))
        finally:
            # The lease starts in a genuine Loop step and spans that Loop's actual
            # completion/abort. No test sets Observation.completed or invents its owner.
            owned.close()
        assert device not in ownership._EAGER
        assert len(request.working_phases) == before + 1
        observation = seen.pop('observation')
        commit = seen.pop('commit')
        row = {'case': name, **seen, 'completion_proven': observation.completion_proven,
               'phase': request.working_phases[-1], 'commit_reason': commit.reason}
        if expect_refusal:
            assert 'refusal' in row
        else:
            assert observation.completion_proven
        try:
            with ownership.eager(commit):
                raise AssertionError('a completed Loop commit was reused')
        except ownership.OwnerConflict:
            row['expired_commit_refused'] = True
        rows.append(row)

    empty = domain.Domain(torch, memory)

    def guarded(_observation, _commit):
        before = vt.stats(device)
        refused = {}
        for name, create in (
            ('domain', empty.initialize),
            ('cache', lambda: memory.region('ownership-proof', 'cache')),
        ):
            try:
                create()
            except ownership.OwnerConflict as error:
                refused[name] = str(error)
            else:
                raise AssertionError(f'{name} created while eager ownership was held')
        results = []

        def another_request():
            try:
                with memory.request('concurrent-ownership-proof'):
                    results.append('unexpectedly-started')
            except ownership.OwnerConflict:
                results.append('refused')

        thread = threading.Thread(target=another_request)
        thread.start()
        thread.join(timeout=10)
        assert not thread.is_alive() and results == ['refused'], results
        assert memory.requests == {request}
        with ownership.preparing(device, 'optional idle preparation', optional=True) as admitted:
            assert not admitted
        after = vt.stats(device)
        for field in ('allocations', 'driver_memory_calls', 'event_records', 'event_queries',
                      'event_waits', 'context_joins'):
            assert after[field] == before[field], (field, before, after)
        return {'blocked_births': refused, 'new_request_refused': True,
                'optional_preparation_deferred': True, 'no_allocator_or_driver_work': True,
                'imported_lm_with_no_engine_allowed': True}

    invoke('empty-owner-inventory-and-birth-exclusion', guarded)
    empty.initialize()  # Genuine library domain and empty CUDA owner graph now exist.
    invoke('actual-captured-domain-refuses-eager', lambda *_: {}, expect_refusal=True)
    empty.finish()
    invoke('retired-domain-allows-eager', lambda *_: {})

    cache = memory.region('ownership-proof-cache', 'fixture', release=lambda: None)
    invoke('actual-cache-registration-refuses-eager', lambda *_: {}, expect_refusal=True)
    cache.close()
    invoke('closed-cache-allows-eager', lambda *_: {})

    with tempfile.TemporaryDirectory(prefix='varena-eager-owner-') as directory:
        writer = tensorfs.Writer(Path(directory) / 'weights.cozytensors')
        writer.component('net').put('weight', torch.full((MIB,), 3.0, dtype=torch.float32))
        checkpoint = writer.commit()
        network = Add()
        vars(network)['_cozy_weights'] = residency.Declared(residency.Weights(checkpoint, 'net'))
        model = memory.load(network, 'ram')
        binding = model.retain(('',))
        try:
            invoke('empty-binding-allows-eager', lambda *_: {})
            with binding.prepare(torch.cuda.current_stream(device)):
                value = network(torch.zeros(MIB, device=memory.device))
                assert int(value[0]) == int(value[-1]) == 3
                del value
                invoke('actual-prepared-binding-refuses-eager', lambda *_: {}, expect_refusal=True)
            binding.close()
            invoke('closed-binding-allows-eager', lambda *_: {})
        finally:
            binding.close()
            model.close()
            checkpoint.close()

    # Hold a real creation claim before a native domain allocation, using explicit events
    # rather than requiring a lucky timing interleave.
    held, release = threading.Event(), threading.Event()
    errors = []
    context = contextvars.copy_context()

    def creator():
        try:
            with ownership.preparing(device, 'concurrent real domain initialization'):
                held.set()
                assert release.wait(10)
                owner = domain.Domain(torch, memory)
                owner.initialize()
                owner.finish()
        except Exception as error:
            errors.append(repr(error))

    thread = threading.Thread(target=context.run, args=(creator,))
    thread.start()
    try:
        assert held.wait(10)
        invoke('creation-in-progress-refuses-eager', lambda *_: {}, expect_refusal=True)
    finally:
        release.set()
        thread.join(timeout=10)
    assert not thread.is_alive() and not errors, errors
    invoke('creation-retired-allows-eager', lambda *_: {})

    # Separate sample completion from proof of the actual owner stream. Each case queues
    # real work, retains an incomplete event, and checks completion AFTER lease retirement.
    for changed_stream in (False, True):
        owned = ExitStack()
        original = torch.cuda.current_stream(device)
        stream = torch.cuda.Stream(device=device)
        other = torch.cuda.Stream(device=device)
        event = torch.cuda.Event()
        state = {}
        # Warm the exact kernel/event before constructing the pending interval.
        with torch.cuda.stream(stream):
            torch.cuda._sleep(1)
            event.record(stream)
        stream.synchronize()
        torch.cuda.set_stream(stream)

        def pending_step(index, value, *, owned=owned, state=state, event=event,
                         stream=stream, other=other, changed_stream=changed_stream):
            del index, value
            observation = phase._ACTIVE.get()
            assert observation is not None and observation.eager_commit is not None
            owned.enter_context(ownership.eager(observation.eager_commit))
            state['observation'] = observation
            torch.cuda._sleep(2_000_000_000)
            event.record(stream)
            assert not event.query(), 'pending-work precondition was not established'
            if changed_stream:
                torch.cuda.set_stream(other)
            else:
                raise StopStep('actual eager abort with queued owner-stream work')

        try:
            try:
                denoise_loop(pending_step, range(1), state=(marker,), key=('pending-owner', changed_stream))
            except StopStep:
                assert not changed_stream
            else:
                assert changed_stream
            observation = state['observation']
            assert not observation.completion_proven
            assert observation.completed == changed_stream
            assert not event.query(), 'unproved owner work already completed before guard close'
            assert device in ownership._EAGER
        finally:
            owned.close()  # Must prove outstanding work before releasing this claim.
            torch.cuda.set_stream(original)
        assert event.query() and device not in ownership._EAGER
        rows.append({'case': 'changed-completion-stream' if changed_stream else 'aborted-owner',
                     'owner_work_was_pending_before_close': True,
                     'owner_work_completed_before_claim_released': True,
                     'sample_completed': observation.completed,
                     'loop_completion_proven': observation.completion_proven,
                     'phase': request.working_phases[-1]})
        stream.synchronize()
        other.synchronize()
        vt.retire_stream(stream, device=device)
        vt.retire_stream(other, device=device)
        del stream, other, event, pending_step, observation
    return rows


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import int8_linear, loop, residency
    from cozy_runtime.internal.execution import ownership
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    assert digest(ownership.__file__) == payload.ownership_sha256
    assert digest(loop.__file__) == payload.loop_sha256
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    device = ctx.device.index or 0
    actual = str(torch.cuda.get_device_properties(device).uuid)
    assert actual.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = residency._REQUEST.get()
    assert isinstance(request, residency.Request) and request.id == ctx.request_id
    assert request.memory.requests == {request} and not request.memory.models and not request.idle
    int8_linear.native(torch, device)
    settle(vt, device)
    initial = vt.stats(device)
    with torch.inference_mode():
        cases = exercise(request, vt, device)
    request.memory.reclaim()
    settle(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert not ownership._EAGER and not ownership._CREATING
    return Report(True, json.dumps({'passed': True, 'cases': cases, 'initial': initial,
        'final': final, 'pid': os.getpid(), 'request_id': ctx.request_id,
        'provenance': msgspec.to_builtins(payload),
        'scope': 'actual componentless Request/Loop commit, real Runtime owners and CUDA completion; no retention policy activation or full H3 eligibility claim'}, sort_keys=True))
