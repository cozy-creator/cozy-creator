"""A fresh independent CUDA participant. It asks; the Runtime donor's own pump services."""
import hashlib
import json
import sys
import time
import traceback
from importlib.metadata import version
from pathlib import Path

config = json.loads(sys.argv[1])

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main():
    import tensorfs
    import torch
    from tensorfs import _ext
    from tensorfs.varena._runtime import Cooperation, CooperationDeferredError
    from tensorfs.varena import torch as vt

    actual = {'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).parent / '_varena_torch.so')}
    assert all(actual[key] == config[key] for key in actual), actual
    assert torch.__version__ == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    actual.update(torch_version=str(torch.__version__), torch_git=torch.version.git_version)
    Cooperation.initialize()
    assert not torch.cuda.is_initialized()
    vt.install(0)
    warm = torch.ones(1, device='cuda')
    assert warm.item() == 1
    del warm
    torch.cuda.synchronize(); vt.trim(0)
    peer = Cooperation(0, config['gpu_uuid'], config['directory'], shared=True, priority=10)

    def retry(operation):
        deadline = time.monotonic() + 15
        while True:
            try:
                return operation()
            except CooperationDeferredError:
                assert time.monotonic() < deadline, 'test operation remained deferred; not a death inference'
                time.sleep(.005)

    try:
        retry(peer.publish)
        emit({'ready': True, 'pid': __import__('os').getpid(), 'provenance': actual})
        assert input() == 'ask'
        deadline = time.monotonic() + 15
        while True:
            table = retry(peer.snapshot)
            donor = next((row for row in table['rows'] if row['pid'] == config['donor_pid']), None)
            if donor is not None and donor['reclaimable'] >= config['wanted']:
                break
            assert time.monotonic() < deadline, {'missing_automatic_publication': table}
            time.sleep(.005)
        before = retry(peer.sample)
        deadline = time.monotonic() + 15
        while True:
            try:
                round_ = peer.ask(before['generation'], config['wanted'])
                break
            except CooperationDeferredError:
                assert time.monotonic() < deadline
                before = retry(peer.sample)
        assert any(row['owner'] == donor['id'] for row in round_['donations']), round_
        deadline = time.monotonic() + 15
        while True:
            table = retry(peer.snapshot)
            answered = table['round']
            assert answered is not None and answered['generation'] == round_['generation'], table
            if answered['finished']:
                break
            assert time.monotonic() < deadline, {'automatic_service_incomplete': table}
            time.sleep(.005)
        donation = next(row for row in answered['donations'] if row['owner'] == donor['id'])
        assert donation['completed'] == config['wanted'] and donation['answer'] == 'Complete', donation
        after = retry(peer.sample)
        assert before['driver_used'] - after['driver_used'] >= config['wanted'], (before, after)
        duplicate = retry(lambda: peer.ask(before['generation'], config['wanted']))
        assert duplicate['generation'] == round_['generation'] and duplicate['completed'] == answered['completed']
        allocated = torch.full((32 << 20,), 29, dtype=torch.uint8, device='cuda')
        assert int(allocated[0]) == int(allocated[-1]) == 29
        during = retry(peer.sample)
        assert during['driver_used'] - after['driver_used'] >= allocated.numel(), (after, during)
        del allocated
        torch.cuda.synchronize(); vt.trim(0)
        final = vt.stats(0)
        assert final['allocated_bytes'] == final['mapped_bytes'] == final['pending_bytes'] == 0, final
        assert final['pools'] == final['event_handles'] == 0, final
        emit({'passed': True, 'donor_before': donor, 'round': answered, 'donation': donation,
              'driver_before': before, 'driver_after': after, 'driver_during_allocation': during,
              'duplicate_round': duplicate, 'requester_allocated_bytes': 32 << 20,
              'requester_final': final})
    finally:
        retry(peer.close)

try:
    main()
except BaseException as error:
    emit({'passed': False, 'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()})
    raise
