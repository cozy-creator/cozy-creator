"""Ordinary GPU app with an identifiable retained process and actual live GPU storage."""
import hashlib
import json
import os
from importlib.metadata import version
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()
TAG = "__APPLICATION__"
HELD = None
CALLS = 0


class Birth(msgspec.Struct):
    pid: int
    start_ticks: int
    boot_id: str


class Prior(msgspec.Struct):
    birth: Birth
    expect: str


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    tensorfs_version: str
    extension_sha256: str
    gpu_uuid: str
    expected_shared: bool | None = None
    prior: list[Prior] = msgspec.field(default_factory=list)
    expected_pid: int | None = None
    expected_parent: Birth | None = None


class Report(msgspec.Struct):
    passed: bool
    detail: str


def identity(pid):
    try:
        stat = Path(f'/proc/{pid}/stat').read_text().rpartition(')')[2].split()
    except FileNotFoundError:
        return None
    return {'pid': pid, 'start_ticks': int(stat[19]),
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(), 'state': stat[0]}


def observation(birth):
    actual = identity(birth.pid)
    same = actual is not None and all(actual[key] == getattr(birth, key)
                                      for key in ('pid', 'start_ticks', 'boot_id'))
    return {'expected': msgspec.to_builtins(birth), 'actual': actual,
            'alive': same and actual['state'] not in ('Z', 'X')}


@app.entrypoint(accelerator=True)
def probe(payload: Probe, ctx: Context) -> Report:
    import torch
    from tensorfs import _ext
    from cozy_runtime.internal.residency import _REQUEST

    global HELD, CALLS
    assert version('cozy-runtime') == payload.runtime_version
    assert version('tensorfs') == payload.tensorfs_version
    assert hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest() == payload.extension_sha256
    request = _REQUEST.get()
    assert request is not None and request.id == ctx.request_id
    owner = request.memory.cooperation
    assert owner is not None and owner.facts()['active'] and owner.config.uuid == payload.gpu_uuid
    prior = []
    for expected in payload.prior:
        assert expected.expect in ('alive', 'ended')
        seen = observation(expected.birth)
        assert seen['alive'] == (expected.expect == 'alive'), seen
        prior.append(seen)
    own, parent = identity(os.getpid()), identity(os.getppid())
    if payload.expected_pid is not None:
        assert own['pid'] == payload.expected_pid, own
    if payload.expected_parent is not None:
        expected = msgspec.to_builtins(payload.expected_parent)
        assert all(parent[key] == value for key, value in expected.items()), (expected, parent)
    if HELD is None:
        HELD = torch.full((16 << 20,), 47, dtype=torch.uint8, device='cuda')
    assert int(HELD[0]) == int(HELD[-1]) == 47
    torch.cuda.synchronize()
    CALLS += 1
    table = owner._peer.snapshot()
    row = next(row for row in table['rows'] if row['id'] == table['owner'])
    if payload.expected_shared is not None:
        assert owner.config.shared == payload.expected_shared
        assert row['sharing'] == ('Shared' if payload.expected_shared else 'Dedicated'), row
    maps = Path(f'/proc/{parent["pid"]}/maps').read_text()
    cmdline = Path(f'/proc/{parent["pid"]}/cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace')
    free, total = torch.cuda.mem_get_info()
    return Report(True, json.dumps({'tag': TAG, 'birth': own, 'parent': parent, 'prior': prior,
        'calls': CALLS, 'storage_ptr': HELD.data_ptr(), 'held_bytes': HELD.numel(),
        'owner_row': table['owner'], 'owner_active': owner.facts()['active'], 'rows': table['rows'],
        'start_shared': owner.config.shared, 'start_priority': owner.config.priority,
        'parent_cmdline': cmdline, 'parent_cuda_loaded': 'libcuda.so' in maps,
        'runtime_version': payload.runtime_version, 'tensorfs_version': payload.tensorfs_version,
        'extension_sha256': payload.extension_sha256, 'driver_free': free, 'driver_total': total}, sort_keys=True))
