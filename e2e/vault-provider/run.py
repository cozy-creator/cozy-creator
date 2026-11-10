#!/usr/bin/env python3
"""Run the real encrypted filesystem fixture, including controller SIGKILL."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
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
    normal = subprocess.run([str(args.probe), 'normal', str(args.securefs), str(args.artifacts / 'normal')], capture_output=True, text=True, env=environment)
    (args.artifacts / 'normal.log').write_text(normal.stdout + normal.stderr)
    if normal.returncode:
        print((normal.stdout + normal.stderr)[-65536:], file=sys.stderr)
        diagnostics = args.artifacts / 'provider.log'
        if diagnostics.exists():
            print(diagnostics.read_text(errors='replace')[-32768:], file=sys.stderr)
        (args.artifacts / 'results.json').write_text(json.dumps({'normal': {'result': 'failed', 'exit_code': normal.returncode}}, indent=2) + '\n')
        raise SystemExit(normal.returncode)
    result = {'normal': json.loads(normal.stdout.strip().splitlines()[-1])}
    crash = args.artifacts / 'crash'
    with (args.artifacts / 'crash.log').open('w') as log:
        host = subprocess.Popen([str(args.probe), 'crash-host', str(args.securefs), str(crash)], stdout=log, stderr=log, env=environment)
        ready = crash / 'ready.json'
        try:
            deadline = time.monotonic() + 90
            while not ready.exists():
                if host.poll() is not None:
                    raise RuntimeError(f'crash fixture exited: {host.returncode}')
                if time.monotonic() > deadline:
                    raise RuntimeError('crash fixture did not become ready')
                time.sleep(0.05)
            view = Path(json.loads(ready.read_text())['view'])
            target = view / 'private-media-input.bin'
            assert target.is_file(), 'the fixture must be readable before controller death'
            host.kill()
            host.wait(timeout=10)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if Path('/proc/self/mountinfo').exists():
                    paths = [line.split()[4].replace('\\040', ' ').replace('\\134', '\\') for line in Path('/proc/self/mountinfo').read_text().splitlines()]
                    mounted = os.path.realpath(view) in paths
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
            result['controller_killed'] = {'result':'pass','view_unmounted':True,'private_reads_refused':True,'fallback_writes_refused':True}
        finally:
            if host.poll() is None:
                host.kill()
                host.wait(timeout=10)
    (args.artifacts / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
