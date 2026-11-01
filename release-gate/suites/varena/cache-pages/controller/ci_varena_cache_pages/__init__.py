"""Ordinary Cozy orchestration of real native Cache proofs; no allocator policy."""
from __future__ import annotations

import ctypes
import hashlib
import importlib.metadata
import json
import os
import select
import shutil
import signal
import subprocess
import tempfile
import time
import traceback
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    source_sha: str
    helper_sha256: str
    native_extension_sha256: str
    gpu_uuid: str
    timeout_s: float = 60.0


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def birth(pid):
    return int(Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()[19])


def setup(payload, ctx, *, cache_required=False):
    import torch
    from tensorfs import _ext

    root = Path(__file__).parent / 'bin'
    binary = root / 'varena-cache-pages'
    manifest = json.loads((root / 'provenance.json').read_text())
    assert manifest['source_sha'] == payload.source_sha
    assert manifest['binary_sha256'] == payload.helper_sha256 == digest(binary)
    assert payload.native_extension_sha256 == digest(_ext.__file__)
    assert manifest['elf']['glibc_max'] <= [2, 28]
    assert not any('python' in name.lower() or 'torch' in name.lower() for name in manifest['elf']['needed'])
    if cache_required:
        assert hasattr(_ext, '_VarenaCache'), 'python_binding requires the new Cache extension'
    own_pidfd = os.pidfd_open(os.getpid())
    try:
        signal.pidfd_send_signal(own_pidfd, 0)
    finally:
        os.close(own_pidfd)
    assert 0 < payload.timeout_s <= 120
    assert ctx.device.type == 'cuda' and (ctx.device.index or 0) == 0
    assert torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    return binary, {'input': msgspec.to_builtins(payload), 'manifest': manifest,
                    'controller_pid': os.getpid(), 'request_id': ctx.request_id,
                    'versions': {name: importlib.metadata.version(name)
                                 for name in ('tensorfs', 'cozy-runtime', 'torch')},
                    'native_extension': str(_ext.__file__),
                    'parent_cache_binding_available': hasattr(_ext, '_VarenaCache'),
                    'driver_before': dict(zip(('free', 'total'), torch.cuda.mem_get_info(0), strict=True))}


def native(payload, ctx, mode):
    result = {'passed': False, 'mode': mode}
    child = pidfd = directory = None
    stdout = stderr = ''
    try:
        binary, provenance = setup(payload, ctx)
        result['provenance'] = provenance
        directory = tempfile.mkdtemp(prefix='varena-cache-pages-', dir='/dev/shm')
        os.chmod(directory, 0o700)
        command = [str(binary), mode, directory, payload.gpu_uuid, '0']
        result['command'] = command
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, close_fds=True, start_new_session=True)
        started = birth(child.pid)
        result.update(pid=child.pid, start_ticks=started)
        pidfd = os.pidfd_open(child.pid)
        assert digest(f'/proc/{child.pid}/exe') == payload.helper_sha256
        deadline = time.monotonic() + payload.timeout_s
        while True:
            ctx.raise_if_cancelled()
            assert time.monotonic() < deadline, 'native proof protocol deadline; no death or release inferred'
            try:
                stdout, stderr = child.communicate(timeout=.05)
                break
            except subprocess.TimeoutExpired:
                continue
        rows = [json.loads(line) for line in stdout.splitlines()]
        result.update(stdout=rows, stderr=stderr, returncode=child.returncode)
        ready = next(row for row in rows if row.get('event') == 'ready')
        assert ready['pid'] == child.pid and ready['start_ticks'] == started
        assert ready['gpu_uuid'] == payload.gpu_uuid and ready['mode'] == mode
        reports = [row for row in rows if row.get('event') == 'result']
        assert len(reports) == 1 and reports[0]['passed'] and child.returncode == 0, result
        for row in (ready, reports[0]):
            assert not row['libraries']['python_loaded'] and not row['libraries']['torch_loaded']
            assert row['libraries']['cuda_loaded']
        result.update(passed=True, report=reports[0])
    except Exception as error:  # noqa: BLE001 - preserve the actual native failure without rerunning it
        result.update(error=f'{type(error).__name__}: {error}', traceback=traceback.format_exc())
    finally:
        if child is not None:
            if child.poll() is None:
                if 'start_ticks' in result:
                    assert birth(child.pid) == result['start_ticks']
                if pidfd is not None:
                    info = Path(f'/proc/self/fdinfo/{pidfd}').read_text()
                    assert next(int(line.split(':', 1)[1]) for line in info.splitlines()
                                if line.startswith('Pid:')) == child.pid
                result['cleanup_signals'] = []
                for kind in (signal.SIGTERM, signal.SIGKILL):
                    if child.poll() is not None:
                        break
                    if pidfd is not None:
                        signal.pidfd_send_signal(pidfd, kind)
                    else:
                        # Startup failure only: this is still our unreaped Popen child,
                        # whose PID cannot be reused. It is no test progress/death proof.
                        child.send_signal(kind)
                    result['cleanup_signals'].append({'pid': child.pid, 'start_ticks': result.get('start_ticks'),
                                                      'signal': kind, 'at_ns': time.monotonic_ns()})
                    try:
                        stdout, stderr = child.communicate(timeout=3)
                    except subprocess.TimeoutExpired:
                        continue
            result['child_exited'] = child.poll() is not None
            if not result.get('stdout'):
                result.update(raw_stdout=stdout, stderr=stderr)
            if pidfd is not None:
                exited = select.poll()
                exited.register(pidfd, select.POLLIN)
                result['pidfd_exit_observed'] = bool(exited.poll(0))
                os.close(pidfd)
            result['returncode'] = child.returncode
            result['passed'] &= result['child_exited'] and child.returncode == 0
        else:
            result['child_exited'] = True
        if directory is not None and result['child_exited']:
            shutil.rmtree(directory)
        elif directory is not None:
            result['preserved_for_owned_cleanup'] = directory
        if 'provenance' in result:
            import torch
            try:
                result['driver_after_child'] = dict(zip(('free', 'total'), torch.cuda.mem_get_info(0), strict=True))
            except Exception as error:  # noqa: BLE001 - retain earlier native failure receipts
                result['driver_after_child_error'] = repr(error)
                result['passed'] = False
    return Report(result['passed'], json.dumps(result, sort_keys=True))


def binding(payload, ctx):
    import torch
    from tensorfs import varena
    from tensorfs.varena import torch as vt

    _, provenance = setup(payload, ctx, cache_required=True)
    before = vt.stats(0)
    cache = None
    try:
        seed = varena.Cache(0, 1)
        page = seed.granularity
        seed.close()
        cache = varena.Cache(0, 3 * page)
        base = cache.base
        cache.commit(0, 3 * page)
        driver = ctypes.CDLL('libcuda.so.1')
        fill = driver.cuMemsetD8_v2
        fill.argtypes, fill.restype = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t], ctypes.c_int
        read = driver.cuMemcpyDtoH_v2
        read.argtypes, read.restype = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t], ctypes.c_int

        def check_page(offset, value, *, write=False):
            if write:
                assert fill(base + offset, value, page) == 0
            block = (ctypes.c_ubyte * page)()
            assert read(block, base + offset, page) == 0
            assert bytes(block) == bytes([value]) * page

        for index, value in enumerate((19, 31, 43)):
            check_page(index * page, value, write=True)
        cache.release(page, page, torch.cuda.current_stream().cuda_stream)
        torch.cuda.synchronize()
        released = cache.poll()
        assert released['mapped_bytes'] == 2 * page and released['pending_bytes'] == 0
        cache.commit(page, page)
        assert cache.base == base
        check_page(page, 79, write=True)
        check_page(0, 19)
        check_page(2 * page, 43)
        during = vt.stats(0)
        assert during['cache_mapped_bytes'] - before['cache_mapped_bytes'] == 3 * page
        cache.release(0, 3 * page, torch.cuda.current_stream().cuda_stream)
        torch.cuda.synchronize()
        cache.poll()
        cache.close()
        cache = None
        after = vt.stats(0)
        for name in ('cache_reserved_bytes', 'cache_mapped_bytes', 'cache_live_bytes',
                     'cache_pending_bytes', 'cache_ready_bytes', 'caches'):
            assert after[name] == before[name], (name, before[name], after[name])
        return Report(True, json.dumps({'passed': True, 'case': 'python_binding', 'provenance': provenance,
                                       'page': page, 'base': base, 'full_readback': True, 'released': released,
                                       'before': before, 'during': during, 'after': after}, sort_keys=True))
    finally:
        if cache is not None:
            # A failed read/driver check does not authorize releasing unknown live consumers.
            # Native deferred finalization retains unproven backing; the failed executor is
            # a failed receipt, never a silently repaired positive result.
            del cache


@app.entrypoint(accelerator=True)
def pages(payload: Probe, ctx: Context) -> Report:
    return native(payload, ctx, 'pages')


@app.entrypoint(accelerator=True)
def physical_rollback(payload: Probe, ctx: Context) -> Report:
    return native(payload, ctx, 'rollback')


@app.entrypoint(accelerator=True)
def abandoned_live(payload: Probe, ctx: Context) -> Report:
    return native(payload, ctx, 'abandoned')


@app.entrypoint(accelerator=True)
def split_choice(payload: Probe, ctx: Context) -> Report:
    return native(payload, ctx, 'choice')


@app.entrypoint(accelerator=True)
def python_binding(payload: Probe, ctx: Context) -> Report:
    return binding(payload, ctx)
