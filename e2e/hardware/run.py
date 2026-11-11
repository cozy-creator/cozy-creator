#!/usr/bin/env python3
"""Run the existing isolated Hub fixture with a private Linux GPU worker home."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent


def processes():
    """A PID plus its Linux birth, executable and HOME; never a process-name match."""
    result = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            stat = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
            env = dict(item.split(b'=', 1) for item in (entry / 'environ').read_bytes().split(b'\0') if b'=' in item)
            result[int(entry.name)] = {
                'birth': stat[19], 'state': stat[0],
                'exe': os.readlink(entry / 'exe'),
                'home': os.fsdecode(env.get(b'HOME', b'')),
                'args': [os.fsdecode(arg) for arg in (entry / 'cmdline').read_bytes().split(b'\0') if arg],
            }
        except (OSError, ValueError):
            pass
    return result


def owned(record, home):
    # Runtime's contained executor gets a HOME below the machine root. A path boundary
    # includes it while excluding similarly named sibling jobs.
    return bool(record['home']) and Path(record['home']).is_relative_to(home) and record['state'] != 'Z'


def same_process(first, second):
    return second is not None and all(first[key] == second[key] for key in ('birth', 'exe', 'home'))


def private_environment(home, shim, device, tools=None):
    # Do not inherit provider, Hub, proxy, auth, dotenv or user service credentials.
    env = {
        'HOME': str(home), 'PATH': str(shim) + os.pathsep + os.environ.get('PATH', os.defpath),
        'LANG': 'C.UTF-8', 'CI': 'true', 'PYTHONUNBUFFERED': '1',
        'XDG_CACHE_HOME': str(home / '.cache'), 'XDG_CONFIG_HOME': str(home / '.config'),
        'XDG_RUNTIME_DIR': str(home / 'run'), 'TMPDIR': str(home / 'tmp'),
        'COZY_MACHINE_ROOT': str(home / '.cozy/machine'),
        'COZY_TENSORFS_ROOT': str(home / 'tensorfs'),
        'CUDA_VISIBLE_DEVICES': device,
        'GOMAXPROCS': '4',
    }
    if tools:
        env['SKEW_TOOLS'] = str(tools)
    return env


def cleanup(home, docker, owner):
    captured = {}
    # Detached Calcifer/TensorD retain this private HOME, including any replacement spawned
    # by a candidate update; contained executors get its descendants. Signal only inspected births.
    for sig in (signal.SIGTERM, signal.SIGKILL):
        snapshot = processes()
        for pid, record in snapshot.items():
            if not owned(record, home):
                continue
            captured[pid] = record
            if processes().get(pid, {}).get('birth') == record['birth']:
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and any(owned(p, home) for p in processes().values()):
            time.sleep(.1)
    query = [docker, 'ps', '-aq', '--filter', f'label=cozy.hardware.owner={owner}']
    containers = subprocess.check_output(query, text=True).split()
    for container in containers:
        subprocess.run([docker, 'rm', '-f', container], check=True, stdout=subprocess.DEVNULL)
    remaining = [pid for pid, p in processes().items() if owned(p, home)]
    return {'processes': captured, 'remaining_pids': remaining, 'containers_removed': containers,
            'remaining_containers': subprocess.check_output(query, text=True).split()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hub-src', type=Path, required=True)
    parser.add_argument('--cozy-bin', type=Path, required=True)
    parser.add_argument('--runtime', required=True, help='Exact Runtime version or wheel path')
    parser.add_argument('--tensorfs', required=True, help='Exact TensorFS version or wheel path')
    parser.add_argument('--device', required=True, help='An explicitly reserved NVIDIA GPU UUID')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--tools', type=Path, help='Directory for the skew fixture MinIO build cache')
    parser.add_argument('--mode', choices=['local-gpu', 'provider-sandbox'], default='local-gpu')
    args = parser.parse_args()
    if args.mode == 'provider-sandbox':
        parser.error('provider lifecycle is unqualified: no provider sandbox account, budget or reachable test allocation is configured; local CUDA is not provider qualification')
    if sys.platform != 'linux' or not args.device.startswith('GPU-'):
        parser.error('this fixture requires Linux and an explicit NVIDIA GPU UUID')
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if any(args.out.iterdir()):
        parser.error('--out must be empty so receipts cannot overwrite another job')
    docker = shutil.which('docker')
    if not docker:
        parser.error('Docker is required for the job-owned PostgreSQL fixture')
    inventory = subprocess.check_output(['nvidia-smi', '--query-gpu=uuid,name', '--format=csv,noheader'], text=True)
    if not any(line.startswith(args.device + ',') for line in inventory.splitlines()):
        parser.error('the selected GPU is not available on this runner')
    busy = subprocess.check_output(['nvidia-smi', '--query-compute-apps=gpu_uuid,pid', '--format=csv,noheader'], text=True)
    if any(line.startswith(args.device + ',') for line in busy.splitlines()):
        parser.error('the selected GPU has an existing compute process; reserve an idle runner')
    original_home = str(Path.home())
    before = {pid: p for pid, p in processes().items()
              if p['home'] == original_home and '/.cozy/' in p['exe']
              and (p['args'][1:] == ['calcifer'] or p['args'][1:] == ['machine', 'serve'])}
    # A short path also keeps executor Unix sockets within Linux's path limit.
    home = Path(tempfile.mkdtemp(prefix='cozy-hardware-'))
    owner = home.name
    shim = home / 'bin'
    for directory in (shim, home / 'run', home / 'tmp', home / '.cache', home / '.config'):
        directory.mkdir(mode=0o700)
    # Tag only this fixture's Docker launches. Cleanup and independent readback use this
    # exact random owner label, so another running fixture can never be selected.
    (shim / 'docker').write_text('#!/usr/bin/env python3\nimport os,sys\na=sys.argv[1:]\n'
        f'cmd={docker!r}\n'
        f'if a and a[0]=="run": a=["run","--label",{("cozy.hardware.owner=" + owner)!r}]+a[1:]\n'
        'os.execv(cmd,[cmd]+a)\n')
    (shim / 'docker').chmod(0o700)
    env = private_environment(home, shim, args.device, args.tools.resolve() if args.tools else None)
    # Public compiler/dependency caches are build inputs; user config and credentials stay absent.
    for name, default in [('GOCACHE', Path(original_home) / '.cache/go-build'),
                          ('GOMODCACHE', Path(original_home) / 'go/pkg/mod'),
                          ('UV_CACHE_DIR', Path(original_home) / '.cache/uv')]:
        env[name] = str(default)
    command = ['bash', str(HERE.parent / 'skew/run.sh'), '--hub-src', str(args.hub_src.resolve()),
               '--cozy-bin', str(args.cozy_bin.resolve()), '--runtime', args.runtime,
               '--tensorfs', args.tensorfs, '--out', str(args.out / 'hub'),
               '--checks', str(HERE / 'checks.sh')]
    receipt = {'mode': args.mode, 'home': str(home), 'owner': owner, 'device': args.device,
               'gpu_inventory': inventory.splitlines(), 'command': command,
               'cozy_sha256': hashlib.sha256(args.cozy_bin.read_bytes()).hexdigest(),
               'hub_source_sha': subprocess.check_output(['git', '-C', str(args.hub_src), 'rev-parse', 'HEAD'], text=True).strip(),
               'runtime': args.runtime, 'tensorfs': args.tensorfs,
               'existing_cozy_processes': before}
    status = 1
    def interrupted(signum, frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        with (args.out / 'console.log').open('w') as log:
            child = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                status = child.wait()
            except KeyboardInterrupt:
                os.killpg(child.pid, signal.SIGTERM)
                status = child.wait()
    finally:
        try:
            receipt['cleanup'] = cleanup(home, docker, owner)
        except Exception as error:
            receipt['cleanup'] = {'error': str(error)}
        after = processes()
        receipt['existing_cozy_processes_unchanged'] = all(same_process(p, after.get(pid)) for pid, p in before.items())
        receipt['fixture_status'] = status
        if not receipt['cleanup'].get('error') and not receipt['cleanup'].get('remaining_pids'):
            shutil.rmtree(home)
        receipt['private_home_removed'] = not home.exists()
        (args.out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    safe = (receipt['existing_cozy_processes_unchanged'] and not receipt['cleanup'].get('error')
            and not receipt['cleanup'].get('remaining_pids') and not receipt['cleanup'].get('remaining_containers'))
    return status if safe else 1


if __name__ == '__main__':
    raise SystemExit(main())
