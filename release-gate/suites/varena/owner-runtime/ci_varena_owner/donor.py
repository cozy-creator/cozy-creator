"""Independent automatic native donor; no explicit ask/publish/service call."""
import ctypes
import gc
import hashlib
import json
import os
import sys
import time
import traceback
from importlib.metadata import version
from pathlib import Path

config = json.loads(sys.argv[1])

def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)

def main():
    import tensorfs
    import torch
    from tensorfs import _ext, varena
    from tensorfs.varena import torch as vt

    def retry(operation):
        deadline = time.monotonic() + 15
        while True:
            try:
                return operation()
            except varena.CooperationDeferredError:
                assert time.monotonic() < deadline, 'native metadata remained deferred'
                time.sleep(.005)

    def digest(path):
        return hashlib.sha256(Path(path).read_bytes()).hexdigest()
    actual = {'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).parent / '_varena_torch.so')}
    assert all(config[key] == value for key, value in actual.items()), actual
    varena.CooperationOwner.initialize()
    vt.install(0)
    warm = torch.full((1,), 7, dtype=torch.uint8, device='cuda')
    assert int(warm[0]) == 7
    del warm
    torch.cuda.synchronize(); vt.trim(0)
    owner = retry(lambda: varena.CooperationOwner(0, config['gpu_uuid'], config['directory'], shared=True, priority=-10))
    live = torch.full((8 << 20,), 23, dtype=torch.uint8, device='cuda')
    idle = torch.full((config['idle_bytes'],), 19, dtype=torch.uint8, device='cuda')
    torch.cuda.synchronize(); del idle; gc.collect()
    end = time.monotonic() + 15
    while True:
        table = retry(owner.snapshot)
        mine = next(row for row in table['rows'] if row['id'] == table['owner'])
        if mine['reclaimable'] >= config['idle_bytes']: break
        assert time.monotonic() < end, mine
        time.sleep(.005)
    emit({'ready': True, 'pid': os.getpid(), 'row': table['owner'], 'before': mine,
          'facts': owner.facts(), 'provenance': actual})
    try:
        for line in sys.stdin:
            command = line.strip()
            if command == 'gil':
                path = Path(__file__).with_name('_gil_gate.so')
                assert digest(path) == config['gil_gate_sha256']
                library = ctypes.PyDLL(str(path))
                library.hold_gil.argtypes = []
                library.hold_gil.restype = ctypes.c_int
                result = library.hold_gil()
                assert result == 0, {'gil_gate_result': result}
                emit({'gil_released': True})
            elif command == 'facts':
                assert int(live[0]) == int(live[-1]) == 23
                table = retry(owner.snapshot)
                mine = next(row for row in table['rows'] if row['id'] == table['owner'])
                emit({'passed': True, 'row': mine, 'facts': owner.facts(), 'live_preserved': True})
            elif command == 'stop':
                break
            else:
                raise AssertionError(command)
    finally:
        live = None
        gc.collect(); torch.cuda.synchronize(); vt.trim(0)
        retry(owner.close)
    final = vt.stats(0)
    assert final['allocated_bytes'] == final['mapped_bytes'] == final['pending_bytes'] == 0, final
    emit({'stopped': True, 'final': final})

try:
    main()
except BaseException as error:
    emit({'passed': False, 'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()})
    raise
