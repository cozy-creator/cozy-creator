#!/usr/bin/env python3
"""Throwaway securefs probe with synthetic data; never touches user projects."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time


def mounted(path):
    return str(path) in [line.split()[4] for line in Path('/proc/self/mountinfo').read_text().splitlines()]


def wait_state(path, wanted, process, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if mounted(path) == wanted:
            return True
        if wanted and process.poll() is not None:
            return False
        time.sleep(0.05)
    return mounted(path) == wanted


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    backing = args.artifacts / 'encrypted'
    view = args.artifacts / 'mounted'
    backing.mkdir(mode=0o700)
    view.mkdir(mode=0o700)
    password = os.urandom(32).hex().encode() + b'\n'
    report = {
        'binary': str(args.binary),
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'version': subprocess.check_output([str(args.binary), 'version'], text=True),
        'artifact_directory': str(args.artifacts),
        'limits': ['Linux only', 'synthetic files', 'no security audit', 'no Creator integration', 'no native GUI player proof'],
        'cases': {},
    }
    process = None
    log = (args.artifacts / 'provider.log').open('wb')

    def start(idle=0):
        proc = subprocess.Popen([str(args.binary), 'mount', '--max-idle-seconds', str(idle), str(backing), str(view)], stdin=subprocess.PIPE, stdout=log, stderr=log)
        proc.stdin.write(password)
        proc.stdin.close()
        if not wait_state(view, True, proc):
            raise RuntimeError('Mount failed; see provider.log')
        return proc

    def unmount():
        return subprocess.run(['fusermount', '-u', str(view)], capture_output=True, text=True)

    try:
        created = subprocess.run([str(args.binary), 'create', '-f', 'full', str(backing)], input=password * 2, capture_output=True)
        (args.artifacts / 'create.log').write_bytes(created.stdout + created.stderr)
        if created.returncode:
            raise RuntimeError('Create failed; see create.log')
        process = start()
        marker = b'XS160_SYNTHETIC_PRIVATE_CONTENT_20261109'
        data = marker + os.urandom(4 * 1024 * 1024)
        filename = 'private-video-input.bin'
        target = view / filename
        with target.open('wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        assert target.read_bytes() == data
        with target.open('rb') as stream:
            stream.seek(3 * 1024 * 1024 + 19)
            assert stream.read(12007) == data[3 * 1024 * 1024 + 19:3 * 1024 * 1024 + 19 + 12007]
        renamed = view / 'renamed-input.bin'
        target.rename(renamed)
        with renamed.open('r+b') as stream:
            stream.seek(1024)
            stream.write(b'UPDATED')
            stream.flush()
            os.fsync(stream.fileno())
        data = data[:1024] + b'UPDATED' + data[1031:]
        assert renamed.read_bytes() == data
        report['cases']['normal_file_io'] = {'result': 'pass', 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'operations': ['create', 'fsync', 'read', 'seek', 'rename', 'overwrite']}
        ffmpeg = ['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24', '-t', '2', '-c:v', 'mpeg4', str(view / 'synthetic.mp4')]
        subprocess.run(ffmpeg, check=True, capture_output=True)
        probe = subprocess.check_output(['ffprobe', '-v', 'error', '-show_entries', 'format=duration,size', '-of', 'json', str(view / 'synthetic.mp4')], text=True)
        subprocess.run(['ffmpeg', '-v', 'error', '-ss', '1', '-i', str(view / 'synthetic.mp4'), '-frames:v', '1', '-f', 'null', '-'], check=True, capture_output=True)
        report['cases']['ordinary_media_program'] = {'result': 'pass', 'ffprobe': json.loads(probe), 'seek_decode': 'one frame at 1 second', 'command': ffmpeg}
        backing_files = [p for p in backing.rglob('*') if p.is_file()]
        assert not any(marker in p.read_bytes() for p in backing_files)
        assert not any('synthetic.mp4' in str(p.relative_to(backing)) or filename in str(p.relative_to(backing)) for p in backing_files)
        report['cases']['backing_inspection'] = {'result': 'pass', 'files': len(backing_files), 'known_plaintext_marker_absent': True, 'known_plaintext_names_absent': True, 'limitation': 'Marker scan is a functional observation, not a cryptographic confidentiality proof.'}
        held = renamed.open('rb', buffering=0)
        held.read(64)
        busy = unmount()
        report['cases']['open_handle_lock'] = {'unmount_returncode': busy.returncode, 'stderr': busy.stderr.strip(), 'still_mounted': mounted(view)}
        held.close()
        if mounted(view):
            done = unmount()
            assert done.returncode == 0, done.stderr
        assert wait_state(view, False, process)
        process.wait(timeout=5)
        assert not (view / 'renamed-input.bin').exists()
        assert not (view / 'synthetic.mp4').exists()
        report['cases']['manual_lock'] = {'result': 'pass', 'mountpoint_remains': view.is_dir(), 'visible_entries': [p.name for p in view.iterdir()]}
        process = start()
        assert (view / 'renamed-input.bin').read_bytes() == data
        report['cases']['remount'] = {'result': 'pass', 'exact_bytes_preserved': True}
        done = unmount()
        assert done.returncode == 0, done.stderr
        process.wait(timeout=5)
        process = start(idle=3)
        started = time.monotonic()
        expired = wait_state(view, False, process, seconds=15)
        report['cases']['idle_expiration'] = {'result': 'pass' if expired else 'failed', 'configured_seconds': 3, 'observed_seconds': round(time.monotonic() - started, 3), 'still_mounted': mounted(view)}
        assert expired, 'Idle expiry did not unmount in observation window'
        process.wait(timeout=5)
        report['result'] = 'pass'
    except Exception as exc:
        report['result'] = 'failed'
        report['error'] = f'{type(exc).__name__}: {exc}'
        raise
    finally:
        if mounted(view):
            unmount()
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        log.close()
        report['cleanup'] = {'mounted': mounted(view), 'provider_running': process is not None and process.poll() is None, 'backing_preserved': backing.exists()}
        (args.artifacts / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
