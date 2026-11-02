"""Actual host identity/inventory stress from an output-only exact native source copy."""
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


def toml(value):
    if isinstance(value, dict):
        return '{' + ', '.join(f'{key} = {toml(item)}' for key, item in value.items()) + '}'
    return json.dumps(value)


manifest = ['[package]', 'name = "varena"', f'version = "{crate["package"]["version"]}"',
            f'edition = "{workspace["package"]["edition"]}"', '[dependencies]']
for name in ('libloading', 'thiserror', 'libc', 'nvml-wrapper'):
    manifest.append(f'{name} = {toml(workspace["dependencies"][name])}')
manifest.append('[features]')
for name, value in crate.get('features', {}).items():
    manifest.append(f'{name} = {toml(value)}')
(fixture / 'Cargo.toml').write_text('\n'.join(manifest) + '\n')
shutil.copy2(args.source / 'Cargo.lock', fixture / 'Cargo.lock')
harnesses = {}
for source, target in [('identity.rs', 'lib.rs'), ('inventory.rs', 'plan.rs')]:
    path = Path(__file__).with_name(source)
    harnesses[source] = hashlib.sha256(path.read_bytes()).hexdigest()
    with (fixture / 'src' / target).open('a') as output:
        output.write('\n' + path.read_text())
(fixture / 'examples').mkdir()
(fixture / 'examples/lifetime_inventory.rs').write_text(
    'fn main() { println!("{}", varena::lifetime_inventory_host_proof()); }\n')
command = ['nice', '-n', '19', 'cargo', 'run', '--offline', '--manifest-path',
           str(fixture / 'Cargo.toml'), '--target-dir', str(args.target), '--example', 'lifetime_inventory', '-j', '1']
start = time.monotonic()
with (args.out / 'stdout').open('w') as stdout, (args.out / 'stderr').open('w') as stderr:
    result = subprocess.run(command, stdout=stdout, stderr=stderr,
                            env={**os.environ, 'CARGO_BUILD_JOBS': '1'}, timeout=180)
report = {'command': command, 'exit_code': result.returncode, 'seconds': time.monotonic() - start,
          'sources': sources, 'harnesses': harnesses,
          'scope': 'real host atomic/registry/fork checks, no actual GPU or fabricated CUDA owners',
          'linkage': 'output-only host executable; not a shipping shared-core/ABI cohort proof'}
if result.returncode == 0:
    report['result'] = json.loads((args.out / 'stdout').read_text())
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(result.returncode)
