"""V3 owner actions with an unrelated real release, actual capped credit and live readback."""
import ctypes
import fcntl
import gc
import json
import os
from pathlib import Path
import threading
import time


class Flock(ctypes.Structure):
    _fields_ = [('type', ctypes.c_short), ('whence', ctypes.c_short),
                ('start', ctypes.c_longlong), ('length', ctypes.c_longlong), ('pid', ctypes.c_int)]


def exercise(payload, ctx):
    from tensorfs import varena
    from . import MIB, Report, Requester, current, eventually

    torch, vt, _memory, owner, provenance = current(payload, ctx)
    native = owner._peer
    assert native.snapshot()['protocol_version'] == 3 and varena.CacheOwner.capability() == 1
    torch.cuda.synchronize(); vt.trim(0)
    initial = vt.stats(0)
    child = cache = None
    handles, callbacks, events = [], [], []
    entered, allow_return = threading.Event(), threading.Event()
    unrelated = live = None
    lock_fd = None
    released = False
    counts = {'decline': 0, 'release': 0}
    driver = ctypes.CDLL('libcuda.so.1')
    fill = driver.cuMemsetD8Async
    fill.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t, ctypes.c_void_p]
    fill.restype = ctypes.c_int
    read = driver.cuMemcpyDtoH_v2
    read.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t]
    read.restype = ctypes.c_int

    def readback(address, size, value):
        data = ctypes.create_string_buffer(size)
        assert read(data, address, size) == 0
        assert data.raw == bytes([value]) * size

    try:
        # Establish the independent participant before opting this fixture Shared, so its
        # context startup is not a new foreign-process signal on host-PID-visible machines.
        eventually(lambda: (native.configure(shared=False, priority=owner.config.priority), True)[1])
        config = {key: getattr(payload, key) for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256', 'gpu_uuid')}
        config.update(directory=str(owner.config.directory), donor_pid=os.getpid(), wanted=32 * MIB, cache_bytes=64 * MIB)
        child = Requester(config, 'action_requester.py'); child_ready = child.read()
        assert child_ready['ready']
        cache = varena.Cache(0, 64 * MIB); cache.commit(0, 64 * MIB)
        stream = torch.cuda.current_stream().cuda_stream
        assert fill(cache.base, 57, 64 * MIB, ctypes.c_void_p(stream)) == 0
        unrelated = torch.full((32 * MIB,), 31, dtype=torch.uint8, device='cuda')
        live = torch.full((8 * MIB,), 23, dtype=torch.uint8, device='cuda')
        torch.cuda.synchronize()
        readback(cache.base, 64 * MIB, 57)

        def decline(wanted):
            counts['decline'] += 1
            assert counts['decline'] == 1
            events.append({'owner': 'decline', 'wanted': wanted, 'entered_ns': time.monotonic_ns()})
            entered.set()
            assert allow_return.wait(10), 'fixture callback release was not supplied'
            return False

        def release(wanted):
            nonlocal released
            counts['release'] += 1
            assert counts['release'] == 1 and wanted == 32 * MIB
            # Ownership/fence only: no poll/close or manual physical reclaim in the callback.
            cache.release(0, 32 * MIB, stream)
            released = True
            events.append({'owner': 'release', 'wanted': wanted, 'returned_ns': time.monotonic_ns()})
            return True

        callbacks.extend([decline, release])  # The native bridge deliberately holds weak refs.
        handles.extend([varena.CacheOwner(0, decline), varena.CacheOwner(0, release)])
        eventually(lambda: (native.configure(shared=True, priority=-10), True)[1])
        child.process.stdin.write(b'ask\n'); child.process.stdin.flush()
        assigned = child.read()
        assert entered.wait(5), 'automatic native owner did not dispatch the first callback'
        table = eventually(native.snapshot)
        path = next(path for path in Path(owner.config.directory).iterdir()
                    if path.is_file() and path.stat().st_ino == table['backing_inode']
                    and path.stat().st_dev == table['backing_device'])
        lock_fd = os.open(path, os.O_RDWR | os.O_CLOEXEC)
        lock = bytes(Flock(fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0))
        end = time.monotonic() + 5
        while True:
            try:
                fcntl.fcntl(lock_fd, fcntl.F_OFD_SETLK, lock); break
            except BlockingIOError:
                assert time.monotonic() < end
                time.sleep(.001)
        before_unrelated = vt.stats(0)
        assert bool(unrelated.eq(31).all().item())
        del unrelated; unrelated = None; gc.collect(); torch.cuda.synchronize(); vt.trim(0)
        after_unrelated = vt.stats(0)
        assert before_unrelated['mapped_bytes'] - after_unrelated['mapped_bytes'] >= 32 * MIB
        assert cache.stats()['mapped_bytes'] == 64 * MIB and not released
        allow_return.set()
        eventually(lambda: True if int(handles[0].facts()['returned']) == 1 else None)
        time.sleep(.12)  # Observe several real cache-dispatcher ticks while service is excluded.
        assert counts['release'] == 0 and cache.stats()['mapped_bytes'] == 64 * MIB
        fcntl.fcntl(lock_fd, fcntl.F_OFD_SETLK, bytes(Flock(fcntl.F_UNLCK, os.SEEK_SET, 0, 1, 0)))
        os.close(lock_fd); lock_fd = None
        child.process.stdin.write(b'observe\n'); child.process.stdin.flush()
        answer = child.read()
        assert answer['passed'] and counts == {'decline': 1, 'release': 1}
        retained = cache.stats()
        assert retained['live_bytes'] == retained['mapped_bytes'] == 32 * MIB, retained
        readback(cache.base + 32 * MIB, 32 * MIB, 57)
        assert bool(live.eq(23).all().item())
        detail = {'provenance': provenance, 'fixture_mode_opt_in': True,
                  'child_ready': child_ready, 'assigned': assigned, 'answer': answer,
                  'callbacks': events, 'callback_counts': counts, 'retained_cache': retained,
                  'before_unrelated_release': before_unrelated, 'after_unrelated_release': after_unrelated,
                  'unrelated_release_did_not_settle_donor': True,
                  'capped_donor_completed_bytes': answer['donation']['completed'],
                  'live_readback_bytes': 40 * MIB, 'initial': initial}
    finally:
        allow_return.set()
        if lock_fd is not None:
            fcntl.fcntl(lock_fd, fcntl.F_OFD_SETLK, bytes(Flock(fcntl.F_UNLCK, os.SEEK_SET, 0, 1, 0)))
            os.close(lock_fd)
        if child is not None: child.close()
        for handle in handles: handle.close()
        unrelated = live = None; gc.collect(); torch.cuda.synchronize()
        if cache is not None:
            if not released: cache.release(0, 32 * MIB, torch.cuda.current_stream().cuda_stream)
            cache.release(32 * MIB, 32 * MIB, torch.cuda.current_stream().cuda_stream)
            eventually(lambda: True if cache.poll()['pending_bytes'] == 0 else None)
            cache.close()
        callbacks.clear()
        eventually(lambda: (native.configure(shared=owner.config.shared, priority=owner.config.priority), True)[1])
        gc.collect(); torch.cuda.synchronize(); vt.trim(0)
    final = vt.stats(0)
    for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles',
                'cache_mapped_bytes', 'cache_reserved_bytes', 'caches'):
        assert final[key] == initial[key], (key, initial, final)
    detail['final'] = final
    return Report(True, json.dumps(detail, sort_keys=True))
