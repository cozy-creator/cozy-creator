"""Real current-layout/rejection proof: native clients and Linux shared files; no CUDA."""
import argparse
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import selectors
import struct
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--peer', required=True, type=Path)
parser.add_argument('--old-peer', required=True, type=Path,
                    help='frozen genuine v2 metadata peer; only a rejection control')
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
actors = []
report = {'gpu_execution': False, 'passed': False, 'cases': {},
          'binaries': {str(p.resolve()): hashlib.sha256(p.read_bytes()).hexdigest()
                       for p in (args.peer, args.old_peer)}}


def run(peer, mode, directory):
    result = subprocess.run([str(peer.resolve()), mode, str(directory)],
                            capture_output=True, text=True, timeout=15)
    return {'status': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}


def actor(peer, mode, directory):
    process = subprocess.Popen([str(peer.resolve()), mode, str(directory)],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, bufsize=1)
    actors.append(process)
    return process


def read(process):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        assert selector.select(15), 'native peer response protocol timeout'
        line = process.stdout.readline()
    assert line, f'peer exited: {process.poll()}'
    return json.loads(line)


def call(process, command):
    process.stdin.write(command + '\n')
    process.stdin.flush()
    return read(process)


def identity(path):
    stat = path.stat()
    return {'dev': stat.st_dev, 'ino': stat.st_ino, 'size': stat.st_size,
            'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def refused_unchanged(directory, path):
    before = identity(path)
    outcomes = [run(args.peer, mode, directory) for mode in ('inspect', 'nonblocking')]
    for outcome in outcomes:
        assert outcome['status'] != 0 and 'incompatible ledger' in outcome['stderr'], outcome
        assert identity(path) == before
    assert len(list(directory.iterdir())) == 1, 'no migration side files'
    return {'before': before, 'after': identity(path), 'refusals': outcomes}


class Flock(ctypes.Structure):
    _fields_ = [('kind', ctypes.c_short), ('whence', ctypes.c_short),
                ('start', ctypes.c_longlong), ('length', ctypes.c_longlong), ('pid', ctypes.c_int)]


try:
    with tempfile.TemporaryDirectory(prefix='varena-current-', dir='/dev/shm') as temp:
        root = Path(temp)
        current = root / 'current'
        current.mkdir()
        owner = actor(args.peer, 'hold', current)
        ready = read(owner)
        path = Path(ready['path'])
        assert call(owner, 'publish 32 64 0 64 1')['ok']
        before = call(owner, 'snapshot')
        observed = run(args.peer, 'inspect', current)
        assert observed['status'] == 0, observed
        assert json.loads(observed['stdout'])['rows'] == before['rows']
        assert call(owner, 'snapshot') == before
        report['cases']['live_current_reopen'] = {'same_snapshot': True, 'inode': path.stat().st_ino}
        owner.stdin.write('close\n'); owner.stdin.flush(); owner.wait(timeout=15)
        assert owner.returncode == 0
        empty = run(args.peer, 'inspect', current)
        assert empty['status'] == 0 and json.loads(empty['stdout'])['rows'] == []
        canonical = path.read_bytes()
        for name, data in (
            ('wrong_version', canonical[:8] + struct.pack('=Q', 2) + canonical[16:]),
            ('wrong_length', canonical[:-8]),
        ):
            path.write_bytes(data)
            report['cases'][name] = refused_unchanged(current, path)
            path.write_bytes(canonical)
        # Recover a writer killed before publishing its initial header. This is current
        # initialization recovery, never permission to erase a nonzero/live header.
        path.write_bytes(bytes(len(canonical)))
        recovered = run(args.peer, 'inspect', current)
        assert recovered['status'] == 0 and json.loads(recovered['stdout'])['rows'] == []
        report['cases']['unpublished_initialization'] = {'passed': True}
        for live in (True, False):
            directory = root / ('old_live' if live else 'old_empty')
            directory.mkdir()
            old = actor(args.old_peer, 'hold' if live else 'observer', directory)
            old_ready = read(old)
            old_path = Path(old_ready['path'])
            if not live:
                old.stdin.write('quit\n'); old.stdin.flush(); old.wait(timeout=15)
                assert old.returncode == 0
            report['cases']['old_live' if live else 'old_empty'] = refused_unchanged(directory, old_path)
            if live:
                assert call(old, 'snapshot')['rows'] == old_ready['table']['rows']
                old.stdin.write('quit\n'); old.stdin.flush(); old.wait(timeout=15)
                assert old.returncode == 0
        # Real blocked opener: verify its actual OFD publication wait in /proc before
        # replacing our owned empty pathname. The opener must refuse the detached inode.
        with path.open('r+b', buffering=0) as locked:
            lock = Flock(fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0)
            fcntl.fcntl(locked.fileno(), fcntl.F_OFD_SETLK, bytes(lock))
            busy = run(args.peer, 'nonblocking', current)
            assert busy['status'] != 0 and 'busy' in busy['stderr'].lower(), busy
            waiting = actor(args.peer, 'inspect', current)
            deadline = time.monotonic() + 15
            while True:
                assert waiting.poll() is None, 'opener left before publication release'
                waiting_on = Path(f'/proc/{waiting.pid}/wchan').read_text().strip()
                if 'fcntl' in waiting_on:
                    break
                assert time.monotonic() < deadline, f'publication-wait protocol timeout: {waiting_on}'
                time.sleep(.01)
            before = identity(path)
            replacement = current / 'replacement'
            replacement.write_bytes(path.read_bytes()); replacement.chmod(0o600)
            replacement.replace(path)
            replacement_identity = identity(path)
            lock.kind = fcntl.F_UNLCK
            fcntl.fcntl(locked.fileno(), fcntl.F_OFD_SETLK, bytes(lock))
            out, err = waiting.communicate(timeout=15)
            assert waiting.returncode != 0 and 'busy' in err.lower(), (out, err)
            assert identity(path) == replacement_identity
            report['cases']['replaced_path'] = {'before': before, 'after': replacement_identity,
                                                'wait': waiting_on, 'error': err}
        assert run(args.peer, 'inspect', current)['status'] == 0
        report['passed'] = True
finally:
    for process in actors:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=15)
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
