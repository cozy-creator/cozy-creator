"""Real checkpoint → Runtime hooks → native RAW admission → demand; no mocked request."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import tempfile
from importlib.metadata import version
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context, denoise_loop
from torch import nn

app = App()
ELEMENTS = 1 << 20
BYTES = ELEMENTS * 4


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    residency_sha256: str
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class Add(nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = nn.Parameter(torch.empty(ELEMENTS, device='meta', dtype=torch.float32), requires_grad=False)

    def forward(self, value):
        return value + self.weight


class Scale:
    identity = 'fixture.actual-scale/2'

    def __init__(self):
        self.calls = 0

    def overlay(self, block, keys):
        return None  # This fixed transform has no separately armed overlay.

    def __call__(self, block, parts):
        self.calls += 1
        for roles in parts.values():
            roles['value'].mul_(2)
        return True


def settle(vt, device):
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def counters(request, vt, device):
    stats = vt.stats(device)
    return {'started': request.prefetch_started, 'completed': request.prefetch_completed,
            'canceled': request.prefetch_canceled, 'declined': request.prefetch_declined,
            'unsupported': dict(request.prefetch_unsupported), 'store_bytes': request.moved,
            'host_bytes': request.baked_bytes,
            **{name: stats[name] for name in
               ('prefetch_attempts', 'prefetch_admissions', 'prefetch_declines')}}


def exercise(request, vt, device):
    from cozy_runtime.internal import residency

    memory = request.memory
    on = memory.device
    assert memory.prefetch_supported
    result = []
    phase_keys = []
    owners = []
    with tempfile.TemporaryDirectory(prefix='runtime-raw-prefetch-') as directory:
        writer = tensorfs.Writer(Path(directory) / 'weights.cozytensors')
        for name in ('plain', 'scaled'):
            component = writer.component(name)
            component.put('0.weight', torch.full((ELEMENTS,), 2.0, dtype=torch.float32, device='cpu'))
            component.put('1.weight', torch.full((ELEMENTS,), 5.0, dtype=torch.float32, device='cpu'))
        checkpoint = writer.commit()
        network = nn.Sequential(Add(), Add())
        vars(network)['_cozy_weights'] = residency.Declared(
            residency.Weights(checkpoint, 'plain'), blocks=['0', '1'],
        )
        loaded = memory.load(network, 'ram')
        owners.append(loaded)
        request.plan([network])
        inputs = torch.zeros(ELEMENTS, dtype=torch.float32, device=on)
        expected = torch.full((ELEMENTS,), 7.0, dtype=torch.float32, device='cpu')
        holder = {}
        key = 'fixture.fixed-adds/1:' + hashlib.sha256(
            repr((loaded.content, loaded.wanted_identity(), ELEMENTS, 'float32',
                  torch.version.git_version, digest(__file__),
                  'two contiguous elementwise adds; no adaptive cuts')).encode()
        ).hexdigest()
        phase_keys.append(key)
        memory.remember_working_phase(key, (loaded,))

        def cold():
            memory.trim()
            settle(vt, device)
            assert memory._prefetch is None
            assert all(not block.cached() for block in loaded.groups())

        def named():
            # This fixture's complete program is fixed before entry. Production Runtime
            # still enters its adaptive eager loops with None; no prior key is guessed here.
            observation = vt.phase_enter(device, key)
            completed = False
            try:
                value = network(inputs)
                torch.cuda.current_stream(device).synchronize()
                completed = True
            finally:
                row = vt.phase_leave(device, observation, completed=completed)
            cpu = value.cpu()
            torch.testing.assert_close(cpu, expected, rtol=0, atol=0)
            return row

        try:
            cold()
            before = counters(request, vt, device)

            def step(index, state, *, network=network):
                del index
                holder['output'] = network(state)

            denoise_loop(step, range(1), state=(inputs,), key=('fixed-adds-unknown-entry',))
            torch.testing.assert_close(holder.pop('output').cpu(), expected, rtol=0, atol=0)
            after = counters(request, vt, device)
            assert after['prefetch_attempts'] > before['prefetch_attempts']
            assert after['prefetch_admissions'] == before['prefetch_admissions']
            assert after['started'] == before['started']
            assert after['declined'] > before['declined']
            assert after['store_bytes'] - before['store_bytes'] == 2 * BYTES
            assert not request.working_phases[-1]['published']
            result.append({'case': 'runtime_unknown_entry', 'before': before, 'after': after,
                           'phase': request.working_phases[-1]})

            cold()
            before = counters(request, vt, device)
            sample = named()
            after = counters(request, vt, device)
            assert sample['published'] and sample['successful_passes'] == 1
            assert after['started'] == before['started']
            assert after['prefetch_admissions'] == before['prefetch_admissions']
            result.append({'case': 'fixed_program_first_sample', 'phase': sample,
                           'before': before, 'after': after})

            cold()
            before = counters(request, vt, device)
            sample = named()
            after = counters(request, vt, device)
            assert sample['published'] and sample['successful_passes'] == 2
            assert after['started'] - before['started'] == 1
            assert after['completed'] - before['completed'] == 1
            assert after['prefetch_admissions'] - before['prefetch_admissions'] == 1
            assert after['store_bytes'] - before['store_bytes'] == 2 * BYTES
            assert memory._prefetch is None
            result.append({'case': 'runtime_known_entry_read_and_use', 'phase': sample,
                           'before': before, 'after': after})

            cold()
            target = loaded.blocks['1']
            before = counters(request, vt, device)
            observation = vt.phase_enter(device, key)
            try:
                # The existing queued-validity boundary cancels the real admitted Fill
                # before its worker starts; this is not a fake read or a timing race.
                queued = residency.QueuedPrefetch('invalidated-fixture-request', (loaded,), lambda: False)
                memory._prefetch_block(target, request, queued=queued)
                assert memory._prefetch is None and target.region is None
                after = counters(request, vt, device)
                assert after['prefetch_admissions'] - before['prefetch_admissions'] == 1
                assert after['started'] == before['started']
                assert after['store_bytes'] == before['store_bytes']
                assert after['canceled'] - before['canceled'] == 1
            finally:
                vt.phase_leave(device, observation, completed=False)
            torch.testing.assert_close(network(inputs).cpu(), expected, rtol=0, atol=0)
            result.append({'case': 'cancel_before_source_io_then_demand',
                           'before': before, 'after': after})

            # Demand populated the existing host-tier wish list. Use its real idle pinning
            # method, then ordinary reclamation; do not manufacture a kept page.
            memory.pin_idle(lambda: False)
            assert memory._baked, 'the real host tier did not retain a page'
            memory.reclaim()
            settle(vt, device)
            before = counters(request, vt, device)
            observation = vt.phase_enter(device, key)
            try:
                memory._prefetch_block(target, request)
                after = counters(request, vt, device)
                assert after['unsupported'].get('kept_host', 0) > before['unsupported'].get('kept_host', 0)
                assert after['prefetch_attempts'] == before['prefetch_attempts']
                assert memory._prefetch is None
            finally:
                vt.phase_leave(device, observation, completed=False)
            before_demand = counters(request, vt, device)
            torch.testing.assert_close(network(inputs).cpu(), expected, rtol=0, atol=0)
            after_demand = counters(request, vt, device)
            assert after_demand['host_bytes'] > before_demand['host_bytes']
            result.append({'case': 'kept_host_declines_demand_uses_host', 'before': before,
                           'after': after, 'demand_before': before_demand, 'demand_after': after_demand})

            transform = Scale()
            scaled = nn.Sequential(Add(), Add())
            vars(scaled)['_cozy_weights'] = residency.Declared(
                residency.Weights(checkpoint, 'scaled'), blocks=['0', '1'], fill=transform,
            )
            scaled_owner = memory.load(scaled, 'ram')
            owners.append(scaled_owner)
            before = counters(request, vt, device)
            memory._prefetch_block(scaled_owner.blocks['1'], request)
            after = counters(request, vt, device)
            assert transform.calls == 0 and memory._prefetch is None
            assert after['unsupported'].get('transform', 0) > before['unsupported'].get('transform', 0)
            assert after['prefetch_attempts'] == before['prefetch_attempts']
            torch.testing.assert_close(scaled(inputs).cpu(), expected * 2, rtol=0, atol=0)
            assert transform.calls == 2
            result.append({'case': 'transform_declines_demand_executes_fill', 'before': before,
                           'after': after, 'demand_transform_calls': transform.calls})
        finally:
            if memory._prefetch is not None:
                memory._settle_prefetch(cancel=True)
            torch.cuda.synchronize(device)
            for owner in reversed(owners):
                owner.close()
            request.plan([])
            checkpoint.close()
        assert not memory.models
        retired = {phase_key: vt.phase_forget(device, phase_key) for phase_key in phase_keys}
        assert not any(retired.values()), retired
        result.append({'case': 'model_owner_retired_phase_keys', 'already_retired': retired})
    return result


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import int8_linear, residency
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    assert digest(residency.__file__) == payload.residency_sha256
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    device = ctx.device.index or 0
    uuid = str(torch.cuda.get_device_properties(device).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = residency._REQUEST.get()
    assert isinstance(request, residency.Request) and request.id == ctx.request_id
    assert request.memory.requests == {request} and not request.memory.models and not request.idle
    assert request.memory.budget_bytes is None
    torch.set_num_threads(1)
    int8_linear.native(torch, device)  # Join the executor's real background probe before baseline.
    gc.collect()
    settle(vt, device)
    initial = vt.stats(device)
    initial_resident = request.memory.native.stats()['resident_bytes']
    with torch.inference_mode():
        cases = exercise(request, vt, device)
    gc.collect()
    request.memory.reclaim()
    settle(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert request.memory.native.stats()['resident_bytes'] == initial_resident == 0
    return Report(True, json.dumps({'passed': True, 'cases': cases, 'initial': initial,
        'final': final, 'pid': os.getpid(), 'request_id': ctx.request_id,
        'provenance': msgspec.to_builtins(payload),
        'scope': 'real executor Request + actual checkpoint/model hooks; explicitly keyed fixed program is separate from unknown adaptive Runtime phases'}, sort_keys=True))
