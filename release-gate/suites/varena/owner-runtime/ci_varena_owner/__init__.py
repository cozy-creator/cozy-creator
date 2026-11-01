"""Real executor-owned cooperation; this handler never calls the donor's publish/service."""
import gc
import hashlib
import json
import os
import selectors
import subprocess
import sys
import time
import traceback
from importlib.metadata import version
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()
MIB = 1 << 20


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_owner_sha256: str
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str


class Observe(Probe, kw_only=True):
    expected_pid: int
    owner_row: dict[str, int]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def current(payload, ctx):
    import tensorfs
    import torch
    from cozy_runtime.internal import cooperation, int8_linear
    from cozy_runtime.internal.residency import _REQUEST
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    request = _REQUEST.get()
    assert request is not None and request.id == ctx.request_id, 'ordinary executor Request required'
    memory = request.memory
    owner = memory.cooperation
    assert owner is not None and owner.pid == os.getpid() and owner.facts()['active']
    assert memory.device.type == 'cuda' and (memory.device.index or 0) == 0
    assert owner.config.uuid == payload.gpu_uuid
    actual = {'runtime_version': version('cozy-runtime'),
              'runtime_owner_sha256': digest(cooperation.__file__),
              'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).parent / '_varena_torch.so')}
    assert all(actual[key] == getattr(payload, key) for key in actual), actual
    assert str(torch.cuda.get_device_properties(0).uuid).removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    assert torch.__version__ == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    int8_linear.native(torch, 0)  # join the executor's existing background readiness owner
    actual.update(pid=os.getpid(), gpu_uuid=payload.gpu_uuid, start_shared=owner.config.shared,
                  start_priority=owner.config.priority, directory=str(owner.config.directory))
    return torch, vt, memory, owner, actual


def eventually(operation):
    from tensorfs.varena import CooperationDeferredError
    deadline = time.monotonic() + 15
    while True:
        try:
            value = operation()
            if value is not None:
                return value
        except CooperationDeferredError:
            pass
        assert time.monotonic() < deadline, 'fixture observation did not arrive; not a process-death inference'
        time.sleep(.005)


class Requester:
    def __init__(self, config):
        self.process = subprocess.Popen([sys.executable, '-I', str(Path(__file__).with_name('requester.py')),
                                         json.dumps(config)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b''
        self.records = []

    def read(self):
        deadline = time.monotonic() + 30
        while b'\n' not in self.buffer:
            left = deadline - time.monotonic()
            assert left > 0 and self.selector.select(left), 'requester protocol did not complete'
            part = os.read(self.process.stdout.fileno(), 65536)
            assert part, {'exit': self.process.poll(), 'stderr': self.process.stderr.read().decode()}
            self.buffer += part
        raw, self.buffer = self.buffer.split(b'\n', 1)
        row = json.loads(raw)
        self.records.append(row)
        assert row.get('passed', True), row
        return row

    def ask(self):
        self.process.stdin.write(b'ask\n'); self.process.stdin.flush()
        return self.read()

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=10)
        self.selector.close()
        self.process.stdin.close(); self.process.stdout.close(); self.process.stderr.close()


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    try:
        return exercise(payload, ctx)
    except Exception as error:  # noqa: BLE001 - Fixture failures are typed evidence, never a retry.
        return Report(False, json.dumps({'passed': False,
            'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()}, sort_keys=True))


def exercise(payload: Probe, ctx: Context) -> Report:
    from tensorfs import varena

    torch, vt, _memory, owner, provenance = current(payload, ctx)
    native = owner._peer  # exact existing owner; metadata/config only, never donor service
    row_id = eventually(native.snapshot)['owner']
    torch.cuda.synchronize(); vt.trim(0)
    initial = vt.stats(0)
    # Dedicated is a valid startup category, not a promise to donate. This explicit fixture
    # owner opts into sharing via the existing native categorical API, then restores Start.
    child = None
    arena = varena.Arena(0)
    idle_region = pinned_region = pinned = live = None
    try:
        eventually(lambda: (native.configure(shared=True, priority=-10), True)[1])
        config = {key: getattr(payload, key) for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256', 'gpu_uuid')}
        config.update(directory=str(owner.config.directory), donor_pid=os.getpid(), wanted=48 * MIB)
        child = Requester(config)
        child_ready = child.read()
        assert child_ready['ready'] and child_ready['pid'] != os.getpid()
        idle_region = arena.reserve(16 * MIB)
        fill = idle_region.prepare(); fill.tensor.fill_(41); fill.commit()
        fill.consume_on(torch.cuda.current_stream().cuda_stream)
        del fill
        pinned_region = arena.reserve(8 * MIB)
        fill = pinned_region.prepare(); fill.tensor.fill_(17); fill.commit()
        fill.consume_on(torch.cuda.current_stream().cuda_stream)
        del fill
        gc.collect(); torch.cuda.synchronize()
        eventually(lambda: True if arena.stats()['pinned_bytes'] == 0 else None)
        pinned = pinned_region.pin(); assert pinned is not None
        live = torch.full((8 * MIB,), 23, dtype=torch.uint8, device='cuda')
        free = torch.full((32 * MIB,), 19, dtype=torch.uint8, device='cuda')
        torch.cuda.synchronize(); del free; gc.collect()
        before = owner.facts()
        # No manual publish/service/wake: the native donor loop must notice peer work even
        # while this ordinary handler is waiting, with no new worker command.
        donated = child.ask()
        child.process.wait(timeout=15)
        assert child.process.returncode == 0
        assert donated['donation']['owner'] == row_id
        after = owner.facts()
        assert after['serviced'] > before['serviced'] and after['published'] > before['published'], (before, after)
        assert arena.stats()['resident_bytes'] == 8 * MIB and not idle_region.resident
        assert int(pinned.tensor[0]) == int(pinned.tensor[-1]) == 17
        assert int(live[0]) == int(live[-1]) == 23
        pinned.consume_on(torch.cuda.current_stream().cuda_stream)
        # Automatic sampling must stay driver-free inside actual capture/replay exclusion.
        checks = []
        for mode in ('capture', 'replay'):
            graph = torch.cuda.CUDAGraph() if mode == 'capture' else None
            stream = torch.cuda.Stream() if graph is not None else None
            scope = vt.graph(graph, stream=stream, reserve_bytes=2 * MIB) if graph is not None else varena.replay(0)
            with scope:
                count = native.cuda_calls(); deferred = owner.facts()['deferred']
                eventually(lambda deferred=deferred: True if owner.facts()['deferred'] > deferred else None)
                assert native.cuda_calls() == count, (mode, count, native.cuda_calls())
            if graph is not None:
                graph.reset(); del graph, stream
            checks.append({'mode': mode, 'automatic_pump_deferred': True, 'native_driver_call_delta': 0})
    finally:
        if child is not None:
            child.close()
        if pinned is not None:
            pinned.consume_on(torch.cuda.current_stream().cuda_stream)
        pinned = live = None
        gc.collect(); torch.cuda.synchronize()
        eventually(lambda: True if arena.stats()['pinned_bytes'] == 0 else None)
        arena.reclaim(1 << 30)
        idle_region = pinned_region = None
        eventually(lambda: (native.configure(shared=owner.config.shared, priority=owner.config.priority), True)[1])
        torch.cuda.synchronize(); vt.trim(0)
    final = vt.stats(0)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert arena.stats()['resident_bytes'] == 0
    actual_row = eventually(lambda: next((row for row in native.snapshot()['rows'] if row['id'] == row_id), None))
    assert actual_row['sharing'] == ('Shared' if owner.config.shared else 'Dedicated')
    return Report(True, json.dumps({'passed': True, 'provenance': provenance, 'owner_row': row_id,
        'automatic_owner_before': before, 'automatic_owner_after': after, 'requester': donated,
        'guards': checks, 'initial': initial, 'final': final,
        'temporary_owner_sharing': True, 'startup_categories_restored': True,
        'scope': 'actual Runtime-created Memory participant and automatic donor pump; no manual donor publish/service; not machine shared-mode propagation, autonomous shortage/foreign-pressure or cache-owner qualification'}, sort_keys=True))


@app.entrypoint(accelerator=True)
def observe(payload: Observe, ctx: Context) -> Report:
    _, _, _, owner, provenance = current(payload, ctx)
    assert os.getpid() == payload.expected_pid, 'a fresh executor is not lifetime evidence'
    table = eventually(owner._peer.snapshot)
    assert table['owner'] == payload.owner_row
    assert owner.facts()['active']
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'owner_row': table['owner'], 'facts': owner.facts()}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup_failure(payload: Probe, ctx: Context) -> Report:
    """Destructive last cell: a real closed native owner must not replace a handler error."""
    from cozy_runtime.internal.cooperation import Failure

    _, _, memory, owner, provenance = current(payload, ctx)
    owner._peer.close()  # Actual native lifetime ends; Runtime still owns its adapter.
    assert not owner.facts()['active']
    try:
        memory.cooperation_boundary()
    except Failure:
        checked_refused = True
    else:
        raise AssertionError('a checked boundary admitted the closed native owner')
    print(json.dumps({'native_owner_cleanup_proof': provenance,
                      'checked_boundary_refused': checked_refused,
                      'expected_error': 'xs138_cleanup_original_error'}, sort_keys=True), flush=True)
    try:
        raise RuntimeError('xs138_cleanup_original_error')
    finally:
        # The real Memory cleanup seam must latch/report the native coordination fault
        # without replacing the exception already unwinding through it.
        memory.cooperation_boundary(check=False)
