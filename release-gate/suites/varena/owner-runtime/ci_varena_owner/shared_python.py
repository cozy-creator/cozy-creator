"""Ordinary Runtime GPU request plus an isolated real final-client/capture check."""
import gc
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def exercise(payload, ctx):
    import tensorfs
    import torch
    from tensorfs import _ext, varena
    from cozy_runtime.internal import cooperation
    from cozy_runtime.internal.residency import _REQUEST
    from . import Report

    request = _REQUEST.get()
    assert request is not None and request.id == ctx.request_id
    actual = {'runtime_version': version('cozy-runtime'),
              'runtime_owner_sha256': hashlib.sha256(Path(cooperation.__file__).read_bytes()).hexdigest(),
              'tensorfs_version': version('tensorfs'),
              'extension_sha256': hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest(),
              'shim_sha256': hashlib.sha256((Path(tensorfs.__file__).parent / '_varena_torch.so').read_bytes()).hexdigest()}
    assert all(actual[key] == getattr(payload, key) for key in actual), actual
    owner = request.memory.cooperation
    assert owner is not None and owner.facts()['active'] and owner.config.uuid == payload.gpu_uuid
    first = owner._peer.snapshot()
    original = next(row for row in first['rows'] if row['id'] == first['owner'])
    baseline = owner.facts()['clients']
    a = b = None
    child = None
    directory = tempfile.mkdtemp(prefix='varena-shared-python-', dir='/dev/shm')
    os.chmod(directory, 0o700)
    try:
        a = varena.CooperationOwner(0, payload.gpu_uuid, owner.config.directory,
                                   shared=not owner.config.shared, priority=owner.config.priority + 71)
        b = varena.CooperationOwner(0, payload.gpu_uuid, owner.config.directory,
                                   shared=not owner.config.shared, priority=owner.config.priority - 71)
        assert a.facts()['owner'] == b.facts()['owner'] == first['owner']
        assert owner.facts()['clients'] == baseline + 2
        current = b.snapshot()
        inherited = next(row for row in current['rows'] if row['id'] == current['owner'])
        assert (inherited['sharing'], inherited['priority']) == (original['sharing'], original['priority'])
        a.close()
        assert a.facts()['closed'] and not a.facts()['active']
        assert b.facts()['active'] and owner.facts()['clients'] == baseline + 1
        value = torch.arange(4096, dtype=torch.int64, device=ctx.device)
        assert int(value.sum()) == 4095 * 4096 // 2
        del value; gc.collect(); torch.cuda.synchronize()
        b.close()
        assert owner.facts()['active'] and owner.facts()['clients'] == baseline
        assert owner._peer.snapshot()['owner'] == first['owner']
        config = {**actual, 'gpu_uuid': payload.gpu_uuid, 'directory': directory}
        command = [sys.executable, '-I', str(Path(__file__).with_name('shared_python_child.py')), json.dumps(config)]
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        stdout, stderr = child.communicate(timeout=120)
        rows = [json.loads(line) for line in stdout.splitlines() if line.strip()]
        assert child.returncode == 0 and len(rows) == 1 and rows[0]['passed'], (rows, stderr, child.returncode)
        assert owner.facts()['active'] and owner._peer.snapshot()['owner'] == first['owner']
        return Report(True, json.dumps({'passed': True, 'provenance': actual, 'pid': os.getpid(),
            'torch': str(torch.__version__), 'torch_cuda': torch.version.cuda,
            'runtime_owner': first['owner'], 'baseline_clients': baseline,
            'inherited_row': inherited, 'nonfinal_continued_gpu_values': 4096,
            'child': rows[0], 'child_returncode': child.returncode,
            'scope': 'ordinary Runtime GPU request with shared Python leases; child proves final-close retry without a permanent Torch root'}, sort_keys=True))
    finally:
        for lease in (a, b):
            if lease is not None:
                lease.close()
        if child is not None and child.poll() is None:
            child.kill(); child.wait(timeout=10)
        # Preserve the owned table files for diagnosis; no live table reset or reinterpretation.
