"""Build a release-gate native concurrency executable from exact product source.

The output-only copy appends the visibility-local lifecycle harness; product source and
its ordinary library artifact stay unchanged. No CUDA participant or Python mock runs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--target', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
fixture = args.out / 'native'
shutil.copytree(args.source / 'crates/varena/src', fixture / 'src')
sources = {str(p.relative_to(args.source)): hashlib.sha256(p.read_bytes()).hexdigest()
           for p in (args.source / 'crates/varena/src').rglob('*.rs')}
workspace = tomllib.loads((args.source / 'Cargo.toml').read_text())['workspace']
crate = tomllib.loads((args.source / 'crates/varena/Cargo.toml').read_text())
package = crate['package']

def toml(value):
    if isinstance(value, dict):
        return '{' + ', '.join(f'{key} = {toml(item)}' for key, item in value.items()) + '}'
    return json.dumps(value)

manifest = ['[package]', 'name = "varena"', f'version = "{package["version"]}"',
            f'edition = "{workspace["package"]["edition"]}"', '[dependencies]']
for name in ('libloading', 'thiserror', 'libc', 'nvml-wrapper'):
    manifest.append(f'{name} = {toml(workspace["dependencies"][name])}')
manifest.append('[features]')
for name, value in crate.get('features', {}).items():
    manifest.append(f'{name} = {toml(value)}')
(fixture / 'Cargo.toml').write_text('\n'.join(manifest) + '\n')
shutil.copy2(args.source / 'Cargo.lock', fixture / 'Cargo.lock')
harness = Path(__file__).with_name('lifecycle.rs')
with (fixture / 'src/cooperative/owner.rs').open('a') as output:
    output.write('\n' + harness.read_text())
with (fixture / 'src/cooperative.rs').open('a') as output:
    output.write('\npub use owner::lifecycle_proof;\n')
(fixture / 'examples').mkdir()
(fixture / 'examples/lifecycle.rs').write_text(
    'fn main() { println!("{}", varena::cooperative::lifecycle_proof()); }\n')
command = ['nice', '-n', '19', 'cargo', 'run', '--offline', '--manifest-path',
           str(fixture / 'Cargo.toml'), '--target-dir', str(args.target), '--example', 'lifecycle', '-j', '1']
start = time.monotonic()
with (args.out / 'stdout').open('w') as stdout, (args.out / 'stderr').open('w') as stderr:
    completed = subprocess.run(command, stdout=stdout, stderr=stderr,
                               env={**os.environ, 'CARGO_BUILD_JOBS': '1'}, timeout=180)
report = {'command': command, 'exit_code': completed.returncode, 'seconds': time.monotonic() - start,
          'sources': sources, 'harness_sha256': hashlib.sha256(harness.read_bytes()).hexdigest(),
          'scope': 'actual native host-control concurrency; no CUDA participant, GPU or native donor qualification'}
if completed.returncode == 0:
    report['result'] = json.loads((args.out / 'stdout').read_text())
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(completed.returncode)
