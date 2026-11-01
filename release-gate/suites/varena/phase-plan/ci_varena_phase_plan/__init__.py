"""Real phase-region declarations, native capacity choices and Runtime source ownership."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import threading
import time
from importlib.metadata import version
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context, denoise_loop
from tensorfs import varena

app = App()
MIB = 1 << 20


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def actual(payload, ctx):
    import cozy_runtime
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    root = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == {
        'internal/residency.py', 'internal/execution/phase.py',
        'internal/execution/native_split.py', 'internal/execution/native_linear_split.py',
        'internal/execution/native_residual_ffn.py', 'internal/encoding/leaves.py',
        'author/kernels.py', 'author/_split.py', '_kernels/_gemm.abi3.so',
    }
    for name, expected in payload.runtime_modules.items():
        assert digest(root / name) == expected, name
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert not request.memory.models and request.memory.budget_bytes is None
    assert request.memory.phase_plan_supported and varena.supports_phase_plans()
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': uuid}


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def touch(region, value, device):
    use = region.prepare()
    tensor = use.tensor
    filled = use.needs_fill
    use.consume_on(torch.cuda.current_stream(device).cuda_stream)
    if filled:
        tensor.fill_(value)
        use.commit()
    torch.cuda.synchronize(device)
    assert int(tensor[0]) == int(tensor[-1]) == value
    result = {'filled': filled, 'pointer': tensor.data_ptr()}
    del tensor, use
    gc.collect()
    torch.cuda.synchronize(device)
    return result


@app.entrypoint(accelerator=True)
def primitive(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal.execution import phase

    request, vt, provenance = actual(payload, ctx)
    device, arena = ctx.device.index or 0, request.memory.native
    on = torch.device('cuda', device)
    assert arena.granularity <= 4 * MIB
    settle(vt, device)
    initial = vt.stats(device)
    initial_weights = arena.stats()
    regions = [arena.reserve(4 * MIB) for _ in range(8)]
    addresses = [region.ptr for region in regions]
    declared = [('all', [*regions, regions[0]]), ('first', regions[:1])]
    copied = varena.Plan(arena, declared)
    declared[0][1].clear()  # the native descriptor was copied, including alias deduplication
    assert copied.facts()['phases'] == [
        {'name': 'all', 'regions': 8, 'bytes': 32 * MIB},
        {'name': 'first', 'regions': 1, 'bytes': 4 * MIB},
    ]
    menu = [('whole', [32 * MIB]), ('half', [16 * MIB]), ('minimum', [8 * MIB])]
    sizes = {'whole': 32 * MIB, 'half': 16 * MIB, 'minimum': 8 * MIB}
    key = 'fixture.phase-regions/whole-half-minimum'
    request.memory.remember_working_phase(key, (request,))
    marker = torch.zeros(1, dtype=torch.int64, device=on)
    cases = []
    try:
        for planned, working in ((False, 16 * MIB), (True, 16 * MIB), (True, 32 * MIB)):
            arena.reclaim(arena.stats()['resident_bytes'])
            settle(vt, device)
            assert touch(regions[0], 1, device)['filled']
            if planned:
                before = arena.stats()
                copied.activate('all')
                assert arena.stats() == before, 'intent changed physical weight ownership'
            else:
                copied.deactivate()
            held = {}

            def step(index, marker, *, planned=planned, working=working, held=held, regions=regions):
                del index, marker
                settle(vt, device)
                before = vt.stats(device)
                # The managed cap is an experimental condition, from actual mapped
                # library storage and native region/declared allocation sizes.
                cap = before['mapped_bytes'] + 32 * MIB + working
                vt.set_limit(device, cap)
                stream = int(torch.cuda.current_stream(device).cuda_stream)
                choice = vt.split_select(device, key, menu, preferred='whole', stream=stream)
                expected = 'half' if planned and working == 16 * MIB else 'whole'
                assert choice['choice_id'] == expected, choice
                assert choice['preserves_phase_weights'] is planned
                if planned:
                    assert choice['phase_weight_bytes'] == 32 * MIB
                    assert choice['phase_missing_bytes'] == 28 * MIB
                phase.choice(('fixture-native-phase-regions', choice['choice_id']))
                observation = phase.begin_split(device, key, choice['choice_id'], choice, stream=stream)
                submitted = False
                try:
                    output = torch.empty(sizes[choice['choice_id']], dtype=torch.uint8, device=on)
                    output.fill_(73)
                    submitted = True
                finally:
                    ended = vt.split_end(device, observation, submitted=submitted)
                touches = [touch(region, i + 1, device) for i, region in enumerate(regions)]
                assert int(output[0]) == int(output[-1]) == 73
                resident = [region.resident for region in regions]
                assert all(resident) is planned, resident
                assert [row['pointer'] for row in touches] == addresses
                held.update(cap=cap, selection=choice, submission=ended,
                            weights=arena.stats(), resident=resident, touches=touches)
                del output

            try:
                denoise_loop(step, range(1), state=(marker,), key=('phase-region-capacity', planned, working))
            finally:
                vt.set_limit(device, None)
            observed = request.working_phases[-1]
            assert len(observed['split_observations']) == 1 and observed['split_observations'][0]['published']
            cases.append({'planned': planned, 'working_bytes': working, **held, 'phase': observed})
            del step, held

        # The caller's explicit selected generation, not mutable last-site state,
        # must reject both phase changes and retirement before any allocation.
        for transition in ('phase', 'deactivate'):
            copied.activate('all')
            choice = vt.split_select(device, key, menu, preferred='whole',
                                     stream=int(torch.cuda.current_stream(device).cuda_stream))
            if transition == 'phase':
                copied.activate('first')
            else:
                copied.deactivate()
            before = vt.stats(device)
            try:
                phase.begin_split(device, key, choice['choice_id'], choice,
                                  stream=int(torch.cuda.current_stream(device).cuda_stream))
            except RuntimeError as error:
                assert 'generation' in str(error), error
            else:
                raise AssertionError('a stale phase-region selection began')
            assert vt.stats(device)['allocations'] == before['allocations']
            cases.append({'transition': transition, 'stale_begin_refused': True, 'allocation_delta': 0})
        copied.activate('all')
        before = arena.stats()
        with varena.capture(device):
            try:
                copied.activate('first')
            except RuntimeError as error:
                assert 'capture' in str(error).lower()
            else:
                raise AssertionError('plan activation entered capture')
        assert arena.stats() == before
        cases.append({'capture_activation_refused': True, 'physical_weights_unchanged': True})
        other = varena.Arena(device)
        foreign = other.reserve(2 * MIB)
        try:
            try:
                varena.Plan(arena, [('cross-arena', [foreign])])
            except ValueError as error:
                assert 'one arena' in str(error)
            else:
                raise AssertionError('a cross-arena descriptor was accepted')
            try:
                copied.activate('all')
            except NotImplementedError as error:
                assert 'one arena' in str(error)
            else:
                raise AssertionError('a second live arena was silently covered')
            cases.append({'cross_arena_descriptor_refused': True,
                          'second_live_arena_activation_refused': True})
        finally:
            del foreign, other
    finally:
        vt.set_limit(device, None)
        copied.deactivate()
        arena.reclaim(arena.stats()['resident_bytes'])
        del copied, declared, regions, marker
        settle(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes']
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert arena.stats()['resident_bytes'] == initial_weights['resident_bytes']
    return Report(True, json.dumps({'passed': True, 'provenance': provenance, 'cases': cases,
                                   'initial': initial, 'final': final,
                                   'request_owned_keys': sorted(request.working_phase_keys)}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))


@app.entrypoint(accelerator=True)
def lifetime(payload: Probe, ctx: Context) -> Report:
    """Race real native selection with the last external Plan owner, then observe retirement."""
    request, vt, provenance = actual(payload, ctx)
    device, arena = ctx.device.index or 0, request.memory.native
    settle(vt, device)
    deadline = time.monotonic() + 10
    before_progress = []
    while arena.stats()['reserved_bytes']:
        current = arena.stats()['reserved_bytes']
        if not before_progress or current != before_progress[-1]:
            before_progress.append(current)
        assert time.monotonic() < deadline, ('prior fixture reservations still owned', before_progress)
        time.sleep(.001)
    before = arena.stats()
    # These reservations have no owner outside the Plan after construction. Their
    # real Region destruction needs capture exclusion, reproducing the ownership
    # relationship that made an in-guard Weak upgrade/drop unsafe.
    ephemeral = varena.Plan(arena, [('ephemeral', [arena.reserve(2 * MIB) for _ in range(64)])])
    ephemeral.activate('ephemeral')
    started, retired, stop = threading.Event(), threading.Event(), threading.Event()
    counts = {'selected': 0, 'generation_refusals': 0}
    errors = []
    key = 'fixture.phase-plan/last-owner'

    def select():
        try:
            for _ in range(10000):
                if stop.is_set():
                    return
                try:
                    found = vt.split_select(device, key, [('probe', [MIB])],
                                            preferred='probe', stream=0)
                except RuntimeError as error:
                    assert any(term in str(error) for term in ('intent changed', 'plan is retiring')), error
                    counts['generation_refusals'] += 1
                    continue
                counts['selected'] += 1
                if found['phase'] == 'ephemeral':
                    started.set()
                elif started.is_set():
                    retired.set()
                    return
            raise AssertionError('retirement was not observed within the bounded native query count')
        except BaseException as error:  # noqa: BLE001 - forward a real thread failure to its owner
            errors.append(f'{type(error).__name__}: {error}')
            retired.set()

    worker = threading.Thread(target=select, daemon=True)
    worker.start()
    try:
        assert started.wait(10), 'native selection never observed the live Plan'
        del ephemeral  # no explicit deactivate; the final native owner must retire safely
        gc.collect()
        assert retired.wait(10), 'native selection or last-owner retirement deadlocked'
        assert not errors, errors
    finally:
        stop.set()
        worker.join(2)
    assert not worker.is_alive(), 'native query did not leave its guards'
    # Actual reservation progress, not a presumed destructor delay, closes ownership.
    deadline = time.monotonic() + 10
    progress = []
    while arena.stats()['reserved_bytes'] != before['reserved_bytes']:
        current = arena.stats()['reserved_bytes']
        if not progress or current != progress[-1]:
            progress.append(current)
        assert time.monotonic() < deadline, ('reservation retirement did not complete', progress)
        time.sleep(.001)
    assert arena.stats()['resident_bytes'] == before['resident_bytes']
    assert vt.phase_forget(device, key)
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'counts': counts, 'prior_retirement_progress': before_progress,
                                   'reservation_progress': progress,
                                   'reservations_retired': 64,
                                   'boundary': 'real concurrent last-owner stress plus checked native retirement; not a timing benchmark or proof of every scheduler interleaving'}, sort_keys=True))


# The real file/model consumer shares the same ordinary App and strict provenance.
from . import runtime as _runtime  # noqa: F401
