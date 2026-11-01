"""Independent v3 requester; never services the Runtime donor or invents release credit."""
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
    Cooperation.initialize(); vt.install(0)
    warm = torch.ones(1, device='cuda'); assert warm.item() == 1
    del warm; torch.cuda.synchronize(); vt.trim(0)
    peer = Cooperation(0, config['gpu_uuid'], config['directory'], shared=True, priority=10)

    def retry(operation):
        end = time.monotonic() + 15
        while True:
            try:
                return operation()
            except CooperationDeferredError:
                assert time.monotonic() < end, 'fixture metadata operation did not settle'
                time.sleep(.005)

    try:
        retry(peer.publish)
        emit({'ready': True, 'pid': os.getpid(), 'provenance': actual})
        assert input() == 'ask'
        end = time.monotonic() + 15
        while True:
            table = retry(peer.snapshot)
            assert table['protocol_version'] == 3
            donor = next((row for row in table['rows'] if row['pid'] == config['donor_pid']), None)
            if donor and donor['requestable'] and donor['physical']['caches']['live'] == config['cache_bytes']:
                break
            assert time.monotonic() < end, table
            time.sleep(.005)
        assert donor['reclaimable'] == 0 and donor['activity'] == 'Active', donor
        before = retry(peer.sample)
        while True:
            try:
                assigned = peer.ask(before['generation'], config['wanted'])
                break
            except CooperationDeferredError:
                assert time.monotonic() < end
                before = retry(peer.sample)
        action = next(row for row in assigned['donations'] if row['owner'] == donor['id'])
        assert assigned['kind'] == 'Allocation' and action['owner_action']
        assert action['offered_at_ask'] == 0 and action['completed'] == 0
        emit({'assigned': assigned, 'donor_before': donor, 'driver_before': before})
        assert input() == 'observe'
        end = time.monotonic() + 15
        while True:
            table = retry(peer.snapshot); round_ = table['round']
            assert round_ is not None and round_['generation'] == assigned['generation'], table
            if round_['finished']:
                break
            assert time.monotonic() < end, table
            time.sleep(.005)
        donation = next(row for row in round_['donations'] if row['owner'] == donor['id'])
        assert donation['answer'] == 'Complete' and donation['completed'] == config['wanted'], donation
        after = retry(peer.sample)
        value = torch.full((config['wanted'],), 29, dtype=torch.uint8, device='cuda')
        assert bool(value.eq(29).all().item())
        del value; torch.cuda.synchronize(); vt.trim(0)
        final = vt.stats(0)
        for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
            assert final[key] == 0, (key, final)
        emit({'passed': True, 'round': round_, 'donation': donation, 'driver_after': after,
              'requester_final': final, 'allocation_readback_bytes': config['wanted']})
    finally:
        retry(peer.close)


try:
    main()
except BaseException as error:
    emit({'passed': False, 'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()})
    raise
