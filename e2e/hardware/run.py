#!/usr/bin/env python3
"""Run the existing isolated Hub fixture with a private Linux GPU worker home."""
import argparse
from array import array
import codecs
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import selectors
import signal
import stat
import subprocess
import sys
import termios
import tempfile
import time
from urllib.request import build_opener, ProxyHandler
from urllib.parse import parse_qs, unquote, urlsplit

HERE = Path(__file__).resolve().parent


def materialize_component(value, directory):
    source = urlsplit(value)
    if not source.scheme:
        return value, {'input': value}
    if source.scheme != 'https':
        raise ValueError('candidate wheel downloads require HTTPS and sha256')
    name = Path(unquote(source.path)).name
    digest = parse_qs(source.fragment).get('sha256', [''])[0]
    if not name.endswith('.whl') or len(digest) != 64 or any(c not in '0123456789abcdef' for c in digest):
        raise ValueError('candidate wheel URL needs its exact sha256 fragment')
    directory.mkdir(parents=True, exist_ok=True)
    destination = directory / name
    partial = directory / (name + '.partial')
    with build_opener(ProxyHandler({})).open(source._replace(fragment='').geturl()) as response, partial.open('wb') as output:
        shutil.copyfileobj(response, output)
    actual = hashlib.sha256(partial.read_bytes()).hexdigest()
    if actual != digest:
        raise ValueError(f'candidate sha256 mismatch: expected {digest}, got {actual}')
    partial.replace(destination)
    return str(destination), {'input': value, 'sha256': digest, 'local_path': str(destination)}


def remove_owned_tree(path):
    """Remove an allocated job tree; repair only its own directories, never link targets."""
    root = Path(path).absolute()
    if not root.exists():
        return
    if root.is_symlink() or root.lstat().st_uid != os.getuid():
        raise PermissionError(f'cleanup root is not an owned directory: {root}')
    try:
        shutil.rmtree(root)
    except PermissionError:
        # Sandboxed package trees and uv Python installations deliberately keep read-only
        # directories. Unlinking their files needs write access on those owned directories.
        def writable(directory):
            metadata = directory.lstat()
            if directory.is_symlink() or metadata.st_uid != os.getuid() or not directory.is_relative_to(root):
                raise PermissionError(f'cannot repair cleanup permissions outside the owned tree: {directory}')
            directory.chmod(stat.S_IMODE(metadata.st_mode) | 0o700, follow_symlinks=False)
        writable(root)
        for directory, children, _ in os.walk(root, followlinks=False):
            for name in children:
                child = Path(directory) / name
                if not child.is_symlink():
                    writable(child)
        shutil.rmtree(root)


def run_logged(command, environment, output):
    with output.open('w') as log:
        child = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 start_new_session=True)
        descriptor = child.stdout.fileno()
        os.set_blocking(descriptor, False)
        selector = selectors.DefaultSelector()
        selector.register(descriptor, selectors.EVENT_READ)
        decoder = codecs.getincrementaldecoder('utf-8')(errors='replace')
        def emit(data, final=False):
            text = decoder.decode(data, final=final)
            log.write(text)
            log.flush()
            print(text, end='', flush=True)
        try:
            while True:
                for _, _ in selector.select(.1):
                    data = os.read(descriptor, 65536)
                    if data:
                        emit(data)
                    else:
                        selector.unregister(descriptor)
                status = child.poll()
                if status is None:
                    continue
                # A login watchdog can retain stdout after the fixture exits. Drain only
                # bytes already queued, rather than waiting for an orphan's eventual EOF.
                queued = array('i', [0])
                fcntl.ioctl(descriptor, termios.FIONREAD, queued, True)
                left = queued[0]
                while left:
                    data = os.read(descriptor, min(left, 65536))
                    if not data:
                        break
                    emit(data)
                    left -= len(data)
                emit(b'', final=True)
                return status
        finally:
            selector.close()
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait()
            child.stdout.close()


def processes():
    """A PID plus its Linux birth, executable and HOME; never a process-name match."""
    result = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            stat = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
            home = next((item[5:] for item in (entry / 'environ').read_bytes().split(b'\0') if item.startswith(b'HOME=')), b'')
            result[int(entry.name)] = {
                'birth': stat[19], 'state': stat[0],
                'exe': os.readlink(entry / 'exe'),
                'home': os.fsdecode(home),
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


def existing_services(snapshot, user_home):
    # Ordinary CLI calls can finish during the job. Preserve the user's serving processes,
    # whose lifetime this harness must not alter, rather than treating completed calls as damage.
    return {pid: p for pid, p in snapshot.items() if p['home'] == user_home and '/.cozy/' in p['exe']
            and (p['args'][1:] == ['calcifer'] or p['args'][1:] == ['machine', 'serve'])}


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
    before = existing_services(processes(), original_home)
    # A short path also keeps executor Unix sockets within Linux's path limit.
    home = Path(tempfile.mkdtemp(prefix='cozy-hardware-'))
    owner = home.name
    shim = home / 'bin'
    for directory in (shim, home / 'run', home / 'tmp', home / '.cache', home / '.config'):
        directory.mkdir(mode=0o700)
    # Tag only this fixture's Docker launches. Cleanup and independent readback use this
    # exact random owner label, so another running fixture can never be selected.
    container_ids = home / 'container-ids'
    (shim / 'docker').write_text('#!/usr/bin/env python3\nimport os,re,subprocess,sys\na=sys.argv[1:]\n'
        f'cmd={docker!r}\n'
        'if a and a[0]=="run":\n'
        f' a=["run","--label",{("cozy.hardware.owner=" + owner)!r}]+a[1:]\n'
        ' result=subprocess.run([cmd]+a,stdout=subprocess.PIPE)\n'
        ' value=result.stdout.decode().strip()\n'
        f' if result.returncode==0 and re.fullmatch("[a-f0-9]{{64}}",value):\n  with open({str(container_ids)!r},"a") as log: log.write(value+"\\n")\n'
        ' sys.stdout.buffer.write(result.stdout)\n sys.exit(result.returncode)\n'
        'os.execv(cmd,[cmd]+a)\n')
    (shim / 'docker').chmod(0o700)
    env = private_environment(home, shim, args.device, args.tools.resolve() if args.tools else None)
    # Public compiler/dependency caches are build inputs; user config and credentials stay absent.
    for name, default in [('GOCACHE', Path(original_home) / '.cache/go-build'),
                          ('GOMODCACHE', Path(original_home) / 'go/pkg/mod'),
                          ('UV_CACHE_DIR', Path(original_home) / '.cache/uv')]:
        env[name] = os.environ.get(name, str(default))
    command = ['bash', str(HERE.parent / 'skew/run.sh'), '--hub-src', str(args.hub_src.resolve()),
               '--cozy-bin', str(args.cozy_bin.resolve()), '--runtime', args.runtime,
               '--tensorfs', args.tensorfs, '--out', str(args.out / 'hub'),
               '--admin', '--command', str(HERE / 'checks.sh')]
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
        receipt['requested_components'] = {}
        for component in ('runtime', 'tensorfs'):
            value, recorded = materialize_component(getattr(args, component), args.out / 'candidates')
            if component == 'tensorfs' and not Path(value).is_file():
                raise ValueError('native SDK qualification requires the exact local or SHA-pinned candidate wheel')
            command[command.index('--' + component) + 1] = value
            receipt['requested_components'][component] = recorded
        status = run_logged(command, env, args.out / 'console.log')
    finally:
        receipt['created_container_ids'] = container_ids.read_text().split() if container_ids.exists() else []
        try:
            receipt['cleanup'] = cleanup(home, docker, owner)
            receipt['cleanup']['remaining_created_containers'] = [container for container in receipt['created_container_ids']
                if subprocess.run([docker, 'inspect', container], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0]
        except Exception as error:
            receipt['cleanup'] = {'error': str(error)}
        after = processes()
        receipt['existing_cozy_processes_unchanged'] = all(same_process(p, after.get(pid)) for pid, p in before.items())
        receipt['fixture_status'] = status
        receipt['private_home_removed'] = False
        # Preserve the verdict even if removal itself fails or the runner is cancelled later.
        (args.out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        if not receipt['cleanup'].get('error') and not receipt['cleanup'].get('remaining_pids'):
            try:
                remove_owned_tree(home)
            except Exception as error:
                receipt['cleanup']['tree_error'] = str(error)
        receipt['private_home_removed'] = not home.exists()
        (args.out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print(f"hardware fixture exit={status}; home removed={receipt['private_home_removed']}; "
              f"existing services preserved={receipt['existing_cozy_processes_unchanged']}", flush=True)
        if receipt['cleanup'].get('error') or receipt['cleanup'].get('tree_error'):
            print(f"hardware cleanup failed: {receipt['cleanup'].get('error') or receipt['cleanup'].get('tree_error')}", flush=True)
    safe = (receipt['existing_cozy_processes_unchanged'] and not receipt['cleanup'].get('error')
            and not receipt['cleanup'].get('remaining_pids') and not receipt['cleanup'].get('remaining_containers')
            and not receipt['cleanup'].get('remaining_created_containers') and receipt['private_home_removed'])
    return status if safe else 1


if __name__ == '__main__':
    raise SystemExit(main())
