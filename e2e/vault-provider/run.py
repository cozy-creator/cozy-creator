#!/usr/bin/env python3
"""Run the real encrypted filesystem fixture, including controller SIGKILL."""
import argparse
import json
import os
import plistlib
import signal
from pathlib import Path
import subprocess
import sys
import time
from typing import TypedDict, cast


class _Session(TypedDict, total=False):
    start_new_session: bool


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--securefs', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    args.probe = args.probe.resolve()
    args.securefs = args.securefs.resolve()
    args.artifacts = args.artifacts.resolve()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    environment = os.environ.copy()
    environment.pop('COZY_SECUREFS_PROVIDER', None)
    environment['COZY_VAULT_PROVIDER_DIAGNOSTICS'] = str(args.artifacts / 'provider.log')
    # Status reads at once: a control command that exits before its waiter reads its birth is no error (cl-1050).
    controls = subprocess.run([str(args.probe), 'controls', str(args.securefs), str(args.artifacts / 'controls')], capture_output=True, text=True, env=environment)
    (args.artifacts / 'controls.log').write_text(controls.stdout + controls.stderr)
    if controls.returncode:
        print((controls.stdout + controls.stderr)[-65536:], file=sys.stderr)
        (args.artifacts / 'results.json').write_text(json.dumps({'controls': {'result': 'failed', 'exit_code': controls.returncode}}, indent=2) + '\n')
        raise SystemExit(controls.returncode)
    controlled = json.loads(controls.stdout.strip().splitlines()[-1])
    if sys.platform == 'linux':
        # A dropped session with a second writer on the supervisor's stdin still ends, view unmounted. A hang is the failure.
        try:
            dropped = subprocess.run([str(args.probe), 'dropped', str(args.securefs), str(args.artifacts / 'dropped')], capture_output=True, text=True, env=environment, timeout=120)
        except subprocess.TimeoutExpired:
            print('dropped: the probe hung past 120 s: the dropped session never ended', file=sys.stderr)
            (args.artifacts / 'results.json').write_text(json.dumps({'dropped': {'result': 'hung'}}, indent=2) + '\n')
            raise SystemExit(1)
        (args.artifacts / 'dropped.log').write_text(dropped.stdout + dropped.stderr)
        if dropped.returncode:
            print((dropped.stdout + dropped.stderr)[-65536:], file=sys.stderr)
            (args.artifacts / 'results.json').write_text(json.dumps({'dropped': {'result': 'failed', 'exit_code': dropped.returncode}}, indent=2) + '\n')
            raise SystemExit(dropped.returncode)
        controlled['dropped'] = json.loads(dropped.stdout.strip().splitlines()[-1])
    normal = subprocess.run([str(args.probe), 'normal', str(args.securefs), str(args.artifacts / 'normal')], capture_output=True, text=True, env=environment)
    (args.artifacts / 'normal.log').write_text(normal.stdout + normal.stderr)
    if normal.returncode:
        print((normal.stdout + normal.stderr)[-65536:], file=sys.stderr)
        diagnostics = args.artifacts / 'provider.log'
        if diagnostics.exists():
            print(diagnostics.read_text(errors='replace')[-32768:], file=sys.stderr)
        (args.artifacts / 'results.json').write_text(json.dumps({'normal': {'result': 'failed', 'exit_code': normal.returncode}}, indent=2) + '\n')
        raise SystemExit(normal.returncode)
    result = {'controls': controlled, 'normal': json.loads(normal.stdout.strip().splitlines()[-1])}
    crash = args.artifacts / 'crash'
    with (args.artifacts / 'crash.log').open('w') as log:
        host = subprocess.Popen([str(args.probe), 'crash-host', str(args.securefs), str(crash)], stdout=log, stderr=log, env=environment, **cast(_Session, {'start_new_session': True} if os.name != 'nt' else {}))  # cast: Popen's one option it may add
        ready = crash / 'ready.json'
        try:
            deadline = time.monotonic() + 90
            while not ready.exists():
                if host.poll() is not None:
                    raise RuntimeError(f'crash fixture exited: {host.returncode}')
                if time.monotonic() > deadline:
                    raise RuntimeError('crash fixture did not become ready')
                time.sleep(0.05)
            recorded = json.loads(ready.read_text())
            view = Path(recorded['view'])
            target = view / 'private-media-input.bin'
            assert target.is_file(), 'the fixture must be readable before controller death'
            if os.name != 'nt':
                assert os.getpgid(recorded['supervisor_pid']) != os.getpgid(host.pid), 'guardian must outlive controller process-group cleanup'
                os.killpg(host.pid, signal.SIGKILL)
            else:
                host.kill()
            host.wait(timeout=10)
            deadline = time.monotonic() + 15
            mounted: bool  # the loop runs at least once: its deadline is ahead
            while time.monotonic() < deadline:
                if Path('/proc/self/mountinfo').exists():
                    paths = [line.split()[4].replace('\\040', ' ').replace('\\134', '\\') for line in Path('/proc/self/mountinfo').read_text().splitlines()]
                    mounted = os.path.realpath(view) in paths
                elif sys.platform == 'darwin':
                    table = subprocess.check_output(['/sbin/mount'], text=True)
                    mounted = f' on {view} (' in table
                    info = plistlib.loads(subprocess.check_output(['/usr/bin/hdiutil', 'info', '-plist']))
                    own_image = (crash / 'encrypted/project.sparsebundle').resolve()
                    attached = any(Path(item.get('image-path', '')).resolve() == own_image for item in info.get('images', []))
                    mounted = mounted or attached
                else:
                    mounted = subprocess.run([str(args.securefs), 'ismount', str(view)], capture_output=True).returncode == 0
                if not mounted:
                    break
                time.sleep(0.05)
            assert not mounted, 'controller SIGKILL must stop its provider'
            try:
                target.read_bytes()
            except OSError:
                pass
            else:
                raise AssertionError('controller SIGKILL left readable private data')
            try:
                (view / 'must-not-leak.bin').write_bytes(b'private')
            except OSError:
                pass
            else:
                raise AssertionError('controller SIGKILL allowed a plaintext fallback write')
            result['controller_killed'] = {'result':'pass','view_unmounted':True,'private_reads_refused':True,'fallback_writes_refused':True,'whole_controller_group':os.name != 'nt'}
        finally:
            if host.poll() is None:
                host.kill()
                host.wait(timeout=10)
    (args.artifacts / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
