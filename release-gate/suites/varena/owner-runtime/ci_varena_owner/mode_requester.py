"""Real independent ledger requester; reports an assignment before waiting for refusal."""
import hashlib
import json
import os
from pathlib import Path
import sys
import time
import traceback
from importlib.metadata import version


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main():
    import tensorfs
    import torch
    from tensorfs import _ext
    from tensorfs.varena import Cooperation, CooperationDeferredError
    from tensorfs.varena import torch as vt

    config = json.loads(sys.argv[1])
    def digest(path):
        return hashlib.sha256(Path(path).read_bytes()).hexdigest()
    actual = {'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).parent / '_varena_torch.so')}
    assert all(actual[key] == config[key] for key in actual), actual
    Cooperation.initialize()
    vt.install(0)
    value = torch.ones(1, device='cuda')
    assert value.item() == 1
    del value
    torch.cuda.synchronize(); vt.trim(0)
    peer = Cooperation(0, config['gpu_uuid'], config['directory'], shared=True, priority=10)

    def retry(operation):
        end = time.monotonic() + 15
        while True:
            try:
                return operation()
            except CooperationDeferredError:
                assert time.monotonic() < end
                time.sleep(.005)

    try:
        retry(peer.publish)
        emit({'ready': True, 'pid': os.getpid(), 'provenance': actual})
        assert input() == 'ask'
        end = time.monotonic() + 15
        while True:
            sample = retry(peer.sample)
            try:
                assigned = peer.ask(sample['generation'], config['wanted'])
                break
            except CooperationDeferredError:
                assert time.monotonic() < end
                continue
        emit({'assigned': assigned})
        assert input() == 'observe'
        end = time.monotonic() + 15
        while True:
            table = retry(peer.snapshot)
            round_ = table['round']
            assert round_['generation'] == assigned['generation']
            if round_['finished']:
                break
            assert time.monotonic() < end
            time.sleep(.005)
        emit({'passed': True, 'round': round_})
    finally:
        retry(peer.close)


try:
    main()
except BaseException as error:
    emit({'passed': False, 'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()})
    raise
