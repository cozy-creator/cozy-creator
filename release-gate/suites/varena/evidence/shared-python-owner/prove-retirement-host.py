"""Output-only exact-source idle queue/fork proof; no GPU resource is fabricated."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib

root = Path(sys.argv[1]).resolve()
out = Path(sys.argv[2]).resolve()
target = Path(sys.argv[3]).resolve()
commit = '49685f0871f429d9003d00687f89d65834c10688'
out.mkdir(parents=True, exist_ok=False)
crate = out / 'native'
sources = {}
for name in subprocess.check_output(['git', 'ls-tree', '-r', '--name-only', commit, 'crates/varena/src'], cwd=root, text=True).splitlines():
    data = subprocess.check_output(['git', 'show', f'{commit}:{name}'], cwd=root)
    path = crate / Path(name).relative_to('crates/varena')
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    sources[name] = hashlib.sha256(data).hexdigest()
workspace = tomllib.loads(subprocess.check_output(['git', 'show', f'{commit}:Cargo.toml'], cwd=root, text=True))['workspace']
package = tomllib.loads(subprocess.check_output(['git', 'show', f'{commit}:crates/varena/Cargo.toml'], cwd=root, text=True))


def toml(value):
    if isinstance(value, dict):
        return '{' + ', '.join(f'{key} = {toml(item)}' for key, item in value.items()) + '}'
    return json.dumps(value)


lines = ['[package]', 'name = "varena"', f'version = "{package["package"]["version"]}"',
         f'edition = "{workspace["package"]["edition"]}"', '[dependencies]']
for name in ('libloading', 'thiserror', 'libc', 'nvml-wrapper'):
    lines.append(f'{name} = {toml(workspace["dependencies"][name])}')
lines.append('[features]')
for key, value in package['features'].items():
    lines.append(f'{key} = {toml(value)}')
(crate / 'Cargo.toml').write_text('\n'.join(lines) + '\n')
(crate / 'Cargo.lock').write_bytes(subprocess.check_output(['git', 'show', f'{commit}:Cargo.lock'], cwd=root))
harness = Path(__file__).with_name('retirement-host-harness.rs')
with (crate / 'src/retirement.rs').open('a') as output:
    output.write('\n' + harness.read_text())
with (crate / 'src/lib.rs').open('a') as output:
    output.write('\npub fn host_queue_proof() -> String { retirement::host_queue_proof() }\n')
(crate / 'examples').mkdir()
(crate / 'examples/host_queue.rs').write_text('fn main() { println!("{}", varena::host_queue_proof()); }\n')
command = ['nice', '-n', '19', 'cargo', '+1.99.1', 'run', '--offline', '-j1', '--manifest-path',
           str(crate / 'Cargo.toml'), '--target-dir', str(target), '--example', 'host_queue']
start = time.monotonic()
with (out / 'stdout').open('w') as stdout, (out / 'stderr').open('w') as stderr:
    result = subprocess.run(command, stdout=stdout, stderr=stderr,
                            env={**os.environ, 'CARGO_BUILD_JOBS': '1'}, timeout=180)
report = {'source': commit, 'sources': sources, 'harness_sha256': hashlib.sha256(harness.read_bytes()).hexdigest(),
          'command': command, 'seconds': time.monotonic() - start, 'exit_code': result.returncode,
          'scope': 'real native host queue startup/coalescence/fork only; no GPU resources constructed or retired',
          'linkage': 'output-only proof executable, not a shipping shared-core ABI cohort'}
if result.returncode == 0:
    report['result'] = json.loads((out / 'stdout').read_text())
(out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(result.returncode)
