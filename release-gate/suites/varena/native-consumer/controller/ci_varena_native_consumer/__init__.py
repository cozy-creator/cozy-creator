"""Python owns only orchestration; every consumer and watcher is the standalone Rust binary."""
from __future__ import annotations

import hashlib
import json
import os
import queue
import select
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import traceback
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()
MIB = 1 << 20


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    helper_sha256: str
    helper_source: str
    native_source: str
    gpu_uuid: str
    donor_bytes: int = 512 * MIB
    request_bytes: int = 384 * MIB
    simultaneous_bytes: int = 768 * MIB
    margin_bytes: int = 128 * MIB
    timeout_s: float = 30.0


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def birth(pid):
    stat = Path(f'/proc/{pid}/stat').read_text()
    return int(stat.rsplit(') ', 1)[1].split()[19])


def pidfd_ready(fd):
    poll = select.poll()
    poll.register(fd, select.POLLIN | select.POLLHUP | select.POLLERR)
    return bool(poll.poll(0))


def mapped_libraries(pid):
    text = Path(f'/proc/{pid}/maps').read_text()
    assert 'libpython' not in text and 'libtorch' not in text, 'native child loaded Python/Torch'
    return {'maps_sha256': hashlib.sha256(text.encode()).hexdigest(),
            'python_loaded': False, 'torch_loaded': False, 'cuda_loaded': 'libcuda.so' in text}


class Child:
    def __init__(self, owner, name, mode, watch=None):
        self.owner, self.name, self.mode = owner, name, mode
        self.rows, self.stderr, self.sent, self.signals = [], [], [], []
        self.pending = queue.Queue()
        self.unread = []
        self.next_id = 0
        self.pidfd = self.gate = None
        self.process = None
        self.ready = None
        read_gate = None
        command = [str(owner.binary), mode, str(owner.directory), owner.payload.gpu_uuid, '0']
        if watch is not None:
            command.append(json.dumps(watch))
        else:
            read_gate, self.gate = os.pipe()
            command.append(str(read_gate))
        self.command = command
        try:
            self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, text=True, bufsize=1,
                                            close_fds=True, pass_fds=() if read_gate is None else (read_gate,),
                                            start_new_session=True)
            owner.children.append(self)  # every successfully spawned process is owned for cleanup
            self.start_ticks = birth(self.process.pid)
            self.pidfd = os.pidfd_open(self.process.pid)
            self.reader = threading.Thread(target=self._stdout, daemon=True)
            self.errors = threading.Thread(target=self._stderr, daemon=True)
            self.reader.start()
            self.errors.start()
            event = 'watch_ready' if watch is not None else 'ready'
            self.ready = self.wait(lambda row: row.get('event') == event)
            assert self.ready['pid'] == self.process.pid and self.ready['start_ticks'] == self.start_ticks
            self.executed_sha256 = digest(f'/proc/{self.process.pid}/exe')
            assert self.executed_sha256 == owner.payload.helper_sha256
            self.maps = mapped_libraries(self.process.pid)
            assert self.maps['cuda_loaded'] is (watch is None)
            self.validate_identity()
        except BaseException:
            if self.process is None and self.gate is not None:
                os.close(self.gate)
                self.gate = None
            raise
        finally:
            if read_gate is not None:
                os.close(read_gate)

    def _stdout(self):
        try:
            for line in self.process.stdout:
                row = json.loads(line)
                libraries = row.get('libraries')
                if libraries is not None:
                    assert not libraries['python_loaded'] and not libraries['torch_loaded']
                    if self.mode == 'watch':
                        assert not libraries['cuda_loaded']
                self.rows.append(row)
                self.pending.put(row)
        except Exception as error:  # noqa: BLE001 - a reader failure must reach the owning request
            self.pending.put({'event': 'reader_error', 'error': repr(error)})
        finally:
            self.pending.put({'event': 'eof'})

    def _stderr(self):
        self.stderr.extend(self.process.stderr)

    def wait(self, predicate, timeout=None):
        for at, row in enumerate(self.unread):
            if predicate(row):
                return self.unread.pop(at)
        deadline = time.monotonic() + (self.owner.payload.timeout_s if timeout is None else timeout)
        while True:
            self.owner.ctx.raise_if_cancelled()
            left = deadline - time.monotonic()
            assert left > 0, f'{self.name}: native protocol deadline; no death inferred'
            try:
                row = self.pending.get(timeout=min(left, .05))
            except queue.Empty:
                continue
            assert row.get('event') not in ('reader_error', 'eof'), (self.name, row, self.stderr)
            if predicate(row):
                return row
            self.unread.append(row)

    def send(self, op, **values):
        self.next_id += 1
        command = {'id': self.next_id, 'op': op, **values}
        self.sent.append({'at_ns': time.monotonic_ns(), 'command': command})
        self.process.stdin.write(json.dumps(command) + '\n')
        self.process.stdin.flush()
        return self.next_id

    def reply(self, command, *, require=True):
        row = self.wait(lambda row: row.get('event') == 'reply' and row.get('id') == command)
        if require:
            assert row['ok'], (self.name, row)
        return row

    def call(self, op, **values):
        deadline = time.monotonic() + self.owner.payload.timeout_s
        while True:
            row = self.reply(self.send(op, **values), require=False)
            if row['ok']:
                return row
            assert op in ('row', 'publish') and row['error_class'] == 'deferred', (self.name, row)
            assert time.monotonic() < deadline, 'metadata remained deferred; no capacity/death inferred'
            time.sleep(.005)

    def validate_identity(self):
        assert self.process.poll() is None
        assert birth(self.process.pid) == self.start_ticks
        assert self.pidfd is not None and not pidfd_ready(self.pidfd)
        info = Path(f'/proc/self/fdinfo/{self.pidfd}').read_text()
        linked = next(int(line.split(':', 1)[1]) for line in info.splitlines() if line.startswith('Pid:'))
        assert linked == self.process.pid
        return {'pid': self.process.pid, 'start_ticks': self.start_ticks, 'pidfd_pid': linked,
                'pidfd_live': True}

    def kill_observed_donor(self, marker, requester):
        assert self.mode == 'manual' and marker['event'] == 'donor_waiting' and marker['read_only']
        assert marker['donor'] == self.ready['row'] and marker['requester'] == requester.ready['row']
        assert marker['pid'] == self.process.pid and marker['start_ticks'] == self.start_ticks
        assert marker['completed'] == marker['pending'] == 0
        requester_identity = requester.validate_identity()
        identity = self.validate_identity()
        record = {'reason': 'exact_observed_waiting_donor', 'marker': marker,
                  'identity': identity, 'requester_identity': requester_identity,
                  'before_ns': time.monotonic_ns(), 'signal': signal.SIGKILL}
        signal.pidfd_send_signal(self.pidfd, signal.SIGKILL)
        record['after_ns'] = time.monotonic_ns()
        self.signals.append(record)
        self.process.wait(timeout=10)
        record['exit_observed_ns'] = time.monotonic_ns()
        assert pidfd_ready(self.pidfd) and self.process.returncode == -signal.SIGKILL
        record['exit_code'] = self.process.returncode
        return record

    def close(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            try:
                self.send('quit')
                self.process.wait(timeout=10)
            except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
                for kind in (signal.SIGTERM, signal.SIGKILL):
                    if self.process.poll() is not None:
                        break
                    # Cleanup is separate from death-test evidence, and only targets our
                    # unreaped Popen child after exact birth/pidfd validation.
                    if self.pidfd is not None:
                        try:
                            identity = self.validate_identity()
                            signal.pidfd_send_signal(self.pidfd, kind)
                        except (AssertionError, FileNotFoundError, ProcessLookupError):
                            if self.process.poll() is not None:
                                break
                            raise
                    else:
                        # Only startup failure can reach this branch. This is our
                        # unreaped child, not a death-test authorization or a donor lookup.
                        assert birth(self.process.pid) == self.start_ticks
                        identity = {'pid': self.process.pid, 'start_ticks': self.start_ticks,
                                    'pidfd_unavailable_during_startup': True}
                        self.process.send_signal(kind)
                    self.signals.append({'reason': 'owned_child_cleanup', 'identity': identity,
                                         'signal': kind, 'at_ns': time.monotonic_ns()})
                    try:
                        self.process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        continue
                assert self.process.poll() is not None, 'owned native child did not terminate'
        for handle in (self.process.stdin, self.process.stdout, self.process.stderr):
            handle.close()
        if hasattr(self, 'reader'):
            self.reader.join(timeout=1)
            self.errors.join(timeout=1)
        for fd in (self.pidfd, self.gate):
            if fd is not None:
                os.close(fd)
        self.pidfd = self.gate = None
        expected_death = any(row['reason'] == 'exact_observed_waiting_donor' for row in self.signals)
        if expected_death:
            assert self.process.returncode == -signal.SIGKILL
        else:
            assert self.process.returncode == 0, (self.name, self.process.returncode, self.stderr)
            if self.mode != 'watch':
                closed = [row for row in self.rows if row.get('event') == 'closed']
                assert len(closed) == 1 and closed[0]['stats']['live'] == 0
                assert closed[0]['stats']['mapped'] == closed[0]['stats']['pending'] == 0

    def receipt(self):
        return {'name': self.name, 'mode': self.mode, 'command': getattr(self, 'command', []),
                'pid': self.process.pid, 'start_ticks': getattr(self, 'start_ticks', None),
                'executed_binary_sha256': getattr(self, 'executed_sha256', None),
                'maps': getattr(self, 'maps', None), 'sent': self.sent, 'rows': self.rows,
                'stderr': ''.join(self.stderr), 'signals': self.signals,
                'returncode': self.process.returncode}


class Harness:
    def __init__(self, payload, ctx, directory):
        import torch

        self.payload, self.ctx, self.directory = payload, ctx, Path(directory)
        self.children = []
        self.binary = Path(__file__).parent / 'bin/varena-native-consumer'
        manifest = json.loads((Path(__file__).parent / 'bin/provenance.json').read_text())
        assert digest(self.binary) == payload.helper_sha256 == manifest['binary_sha256']
        assert manifest['helper_source'] == payload.helper_source
        assert manifest['native_source'] == payload.native_source
        assert manifest['elf']['glibc_max'] <= [2, 28]
        assert not any('python' in name.lower() or 'torch' in name.lower() for name in manifest['elf']['needed'])
        assert hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal')
        own_pidfd = os.pidfd_open(os.getpid())
        try:
            signal.pidfd_send_signal(own_pidfd, 0)
        finally:
            os.close(own_pidfd)
        assert ctx.device.type == 'cuda' and (ctx.device.index or 0) == 0
        assert torch.cuda.device_count() == 1
        uuid = str(torch.cuda.get_device_properties(0).uuid)
        assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
        assert payload.gpu_uuid.startswith('GPU-')
        assert 0 < payload.timeout_s <= 60
        assert 0 < payload.margin_bytes < payload.request_bytes < payload.donor_bytes
        assert payload.donor_bytes < payload.simultaneous_bytes
        assert all(n % (2 * MIB) == 0 for n in (payload.margin_bytes, payload.request_bytes,
                                               payload.donor_bytes, payload.simultaneous_bytes))
        self.provenance = {**msgspec.to_builtins(payload), 'controller_pid': os.getpid(),
                           'request_id': ctx.request_id, 'binary': str(self.binary), 'build': manifest,
                           'ledger_directory': str(self.directory),
                           'parent_driver_before': dict(zip(('free', 'total'), torch.cuda.mem_get_info(0), strict=True))}

    def spawn(self, name, mode, watch=None):
        return Child(self, name, mode, watch)

    def close(self):
        errors = []
        for child in reversed(self.children):
            try:
                child.close()
            except Exception as error:  # noqa: BLE001 - preserve every cleanup failure and continue other owned children
                errors.append(f'{child.name}: {type(error).__name__}: {error}')
        return errors


def warm(child):
    allocation = child.call('allocate', name='warm', bytes=2 * MIB, value=17)
    verified = child.call('verify', name='warm')
    assert verified['result']['bytes_verified'] == 2 * MIB
    child.call('free', name='warm')
    child.call('trim')
    child.call('allocate', name='canary', bytes=2 * MIB, value=29)
    child.call('verify', name='canary')
    return {'allocation': allocation, 'verified': verified}


def pressure(filler, margin):
    reading = filler.call('stats')
    free = reading['driver']['free']
    size = (free - margin) // (2 * MIB) * (2 * MIB)
    assert size > 0, {'free': free, 'margin': margin}
    allocation = filler.call('allocate', name='pressure', bytes=size, value=53)
    verified = filler.call('verify', name='pressure')
    assert verified['result']['bytes_verified'] == size
    return {'observed_free': free, 'explicit_margin': margin, 'requested_filler_bytes': size,
            'allocation': allocation, 'full_readback': verified}


def smoke_case(h):
    results = []
    for mode in ('auto', 'manual', 'plain'):
        child = h.spawn(mode, mode)
        warmed = warm(child)
        child.call('allocate', name='full-readback', bytes=64 * MIB, value=71)
        verified = child.call('verify', name='full-readback')
        assert verified['result']['bytes_verified'] == 64 * MIB
        child.call('free', name='full-readback')
        if mode == 'manual':
            published = child.call('publish')
            assert published['result']['refreshed']
            assert published['result']['reclaimable'] >= 64 * MIB
        results.append({'mode': mode, 'warm': warmed, 'full_readback': verified})
    return {'cases': results, 'scope': 'all native modes; complete byte readback, no Python/Torch maps'}


def death_case(h):
    donor, requester, filler = h.spawn('donor', 'manual'), h.spawn('requester', 'auto'), h.spawn('filler', 'plain')
    for child in (donor, requester, filler):
        warm(child)
    donor.call('allocate', name='idle', bytes=h.payload.donor_bytes, value=41)
    donor.call('verify', name='idle')
    donor.call('free', name='idle')
    published = donor.call('publish')  # then its main thread blocks; no service command follows
    assert published['result']['refreshed'] and published['result']['reclaimable'] >= h.payload.donor_bytes
    assert published['stats']['pending'] == 0
    setup = pressure(filler, h.payload.margin_bytes)
    assert setup['full_readback']['driver']['free'] < h.payload.request_bytes
    watcher = h.spawn('watcher', 'watch', {
        'donor_slot': donor.ready['row']['slot'], 'donor_incarnation': donor.ready['row']['incarnation'],
        'requester_slot': requester.ready['row']['slot'],
        'requester_incarnation': requester.ready['row']['incarnation'], 'timeout_ms': 5000,
    })
    before = requester.call('stats')
    command = requester.send('allocate', name='demand', bytes=h.payload.request_bytes, value=83)
    marker = watcher.wait(lambda row: row.get('event') == 'donor_waiting', timeout=6)
    killed = donor.kill_observed_donor(marker, requester)
    first = requester.reply(command, require=False)
    interval_end = next(row for row in requester.rows if row.get('event') == 'allocation_end' and row.get('id') == command)
    assert marker['at_ns'] <= killed['before_ns'] < interval_end['at_ns'], 'death did not occur during the actual allocator call'
    assert first['stats']['driver_physical_ooms'] > before['stats']['driver_physical_ooms']
    assert first['owner']['recoveries'] > before['owner']['recoveries']
    assert first['owner']['last_recovery']['completed_bytes'] == 0, 'a dead row was credited as released bytes'
    result = {'published_idle': published, 'pressure': setup, 'watch_ready': watcher.ready,
              'waiting_marker': marker, 'owned_death': killed, 'initial_allocation': first,
              'post_operation_metadata': requester.call('row')}
    after_round = result['post_operation_metadata']['result']['round']
    if after_round is not None and after_round['generation'] == marker['round']:
        donation = next(d for d in after_round['donations'] if d['owner'] == donor.ready['row'])
        assert donation['completed'] == 0, donation
    if first['ok']:
        result['initial_outcome'] = 'allocated_after_actual_driver_retry'
        result['full_readback'] = requester.call('verify', name='demand')
        assert result['full_readback']['result']['bytes_verified'] == h.payload.request_bytes
    else:
        assert first['error_class'] == 'capacity', first
        result['initial_outcome'] = 'physical_exhaustion_preserved_after_death'
        observed = requester.call('stats')
        result['post_exit_driver_observation'] = observed
        if observed['driver']['free'] >= h.payload.request_bytes + h.payload.margin_bytes:
            later = requester.reply(requester.send('allocate', name='after-exit', bytes=h.payload.request_bytes, value=89), require=False)
            result['separate_post_exit_allocation'] = later
            if later['ok']:
                result['separate_post_exit_readback'] = requester.call('verify', name='after-exit')
                assert result['separate_post_exit_readback']['result']['bytes_verified'] == h.payload.request_bytes
            else:
                assert later['error_class'] == 'capacity', later
        else:
            result['separate_post_exit_allocation'] = 'not attempted: driver headroom still insufficient'
    requester.call('verify', name='canary')
    filler.call('verify', name='canary')
    result['boundary'] = 'Waiting row plus owned PID/birth/pidfd authorizes this fixture death; metadata reaping grants no capacity. Initial failure and any later driver-backed readback remain separate.'
    return result


def simultaneous_case(h):
    left, right, filler = h.spawn('left', 'auto'), h.spawn('right', 'auto'), h.spawn('filler', 'plain')
    peers = (left, right)
    for child in (*peers, filler):
        warm(child)
    published = []
    for child in peers:
        child.call('allocate', name='idle', bytes=h.payload.donor_bytes, value=47)
        child.call('free', name='idle')
        deadline = time.monotonic() + 5
        while True:
            row = child.call('row')  # metadata snapshot only; no publish/service/ask command
            if row['result']['reclaimable'] >= h.payload.donor_bytes and row['result']['working']['idle'] >= h.payload.donor_bytes:
                published.append(row)
                break
            assert time.monotonic() < deadline, 'automatic owner did not publish actual idle cache'
            time.sleep(.01)
    setup = pressure(filler, h.payload.margin_bytes)
    assert setup['full_readback']['driver']['free'] < h.payload.simultaneous_bytes
    commands = [child.send('work', name='simultaneous', bytes=h.payload.simultaneous_bytes,
                           value=97 + i, gated=True) for i, child in enumerate(peers)]
    armed = [child.wait(lambda row, command=command: row.get('event') == 'operation_ready' and row.get('id') == command)
             for child, command in zip(peers, commands, strict=True)]
    for child in peers:
        child.validate_identity()
    released = time.monotonic_ns()
    for child in peers:
        os.write(child.gate, b'\x01')
    replies = [child.reply(command, require=False) for child, command in zip(peers, commands, strict=True)]
    intervals = []
    for child, command, row in zip(peers, commands, replies, strict=True):
        begin = next(r['at_ns'] for r in child.rows if r.get('event') == 'allocation_begin' and r.get('id') == command)
        end = next(r['at_ns'] for r in child.rows if r.get('event') == 'allocation_end' and r.get('id') == command)
        intervals.append((begin, end))
        if row['ok']:
            assert row['result']['bytes_verified'] == h.payload.simultaneous_bytes and row['result']['freed']
        else:
            assert row['error_class'] == 'capacity', row
        child.call('verify', name='canary')
    assert max(start for start, _ in intervals) < min(end for _, end in intervals), intervals
    assert sum(r['stats']['driver_physical_ooms'] for r in replies) > sum(r['stats']['driver_physical_ooms'] for r in published)
    assert any(r['owner']['recoveries'] for r in replies), replies
    final_rows = [child.call('row') for child in peers]
    return {'initial_published_idle': published, 'pressure': setup, 'operation_ready': armed,
            'gates_released_ns': released, 'actual_allocation_intervals_ns': intervals,
            'overlap_proven': True, 'replies': replies, 'final_owner_rows': final_rows,
            'outcomes': ['full_readback_then_free' if r['ok'] else 'accepted_physical_exhaustion' for r in replies],
            'boundary': 'Both real automatic consumers hold idle cache then demand concurrently. Actual recovery/donation facts are recorded; equal success or donation by both is not inferred.'}


def execute(payload, ctx, case):
    h = None
    result = {'passed': False, 'case': case.__name__}
    directory = tempfile.mkdtemp(prefix='varena-native-consumers-')
    os.chmod(directory, 0o700)
    try:
        h = Harness(payload, ctx, directory)
        result.update(provenance=h.provenance, detail=case(h), passed=True)
    except Exception as error:  # noqa: BLE001 - preserve native failure receipts, never retry the cell
        result.update(error=f'{type(error).__name__}: {error}', traceback=traceback.format_exc())
    finally:
        failures = [] if h is None else h.close()
        if h is not None:
            result['children'] = [child.receipt() for child in h.children]
        result['cleanup_errors'] = failures
        if failures:
            result['passed'] = False
        gone = h is None or all(child.process.poll() is not None for child in h.children)
        result['all_owned_children_exited'] = gone
        if h is not None:
            import torch
            try:
                result['parent_driver_after_children'] = dict(zip(('free', 'total'), torch.cuda.mem_get_info(0), strict=True))
            except Exception as error:  # noqa: BLE001 - preserve the original native receipts on a driver failure
                result['parent_driver_after_children_error'] = repr(error)
                result['passed'] = False
        if gone:
            shutil.rmtree(directory)
        else:
            result['preserved_directory_for_owned_cleanup'] = directory
            result['passed'] = False
    return Report(result['passed'], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def smoke(payload: Probe, ctx: Context) -> Report:
    return execute(payload, ctx, smoke_case)


@app.entrypoint(accelerator=True)
def donor_death(payload: Probe, ctx: Context) -> Report:
    return execute(payload, ctx, death_case)


@app.entrypoint(accelerator=True)
def simultaneous_auto(payload: Probe, ctx: Context) -> Report:
    return execute(payload, ctx, simultaneous_case)
