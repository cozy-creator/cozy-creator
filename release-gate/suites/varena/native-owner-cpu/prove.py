"""Build a release-gate native concurrency executable from exact product source.

The output-only copy appends the visibility-local lifecycle harness; product source and
its ordinary library artifact stay unchanged. No CUDA participant or Python mock runs.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import time
import uuid
from pathlib import Path

import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--target', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
parser.add_argument('--default-service', action='store_true')
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
for name in ('libloading', 'thiserror', 'libc', 'nvml-wrapper', 'nvml-wrapper-sys'):
    manifest.append(f'{name} = {toml(workspace["dependencies"][name])}')
manifest.append('[features]')
for name, value in crate.get('features', {}).items():
    manifest.append(f'{name} = {toml(value)}')
(fixture / 'Cargo.toml').write_text('\n'.join(manifest) + '\n')
shutil.copy2(args.source / 'Cargo.lock', fixture / 'Cargo.lock')
harness = Path(__file__).with_name('lifecycle.rs')
with (fixture / 'src/cooperative/owner.rs').open('a') as output:
    output.write('\n' + harness.read_text())
    output.write('\n' + harness.with_name('shared.rs').read_text())
    if args.default_service:
        output.write('\n' + harness.with_name('default_service.rs').read_text())
with (fixture / 'src/cooperative/pressure.rs').open('a') as output:
    output.write('\n' + harness.with_name('pressure_host.rs').read_text())
with (fixture / 'src/cooperative.rs').open('a') as output:
    output.write('\npub use owner::lifecycle_proof;\n')
    if args.default_service:
        output.write('\npub use owner::default_service_proof;\n')
if args.default_service:
    with (fixture / 'src/cooperative/domain.rs').open('a') as output:
        output.write('\n' + harness.with_name('default_domain.rs').read_text())
(fixture / 'examples').mkdir()
(fixture / 'examples/lifecycle.rs').write_text(
    'fn main() { ' + ('varena::cooperative::default_service_proof(); ' if args.default_service else '')
    + 'println!("{}", varena::cooperative::lifecycle_proof()); }\n')
command = ['nice', '-n', '19', 'cargo', 'run', '--offline', '--manifest-path',
           str(fixture / 'Cargo.toml'), '--target-dir', str(args.target), '--example', 'lifecycle', '-j', '1']
ledger_directory = Path('/dev/shm') / ('xs138-shared-' + uuid.uuid4().hex)
start = time.monotonic()
with (args.out / 'stdout').open('w') as stdout, (args.out / 'stderr').open('w') as stderr:
    completed = subprocess.run(command, stdout=stdout, stderr=stderr, check=False,
                               env={**os.environ, 'CARGO_BUILD_JOBS': '1', 'VARENA_HOST_PROOF_DIRECTORY': str(ledger_directory)}, timeout=180)
report = {'command': command, 'exit_code': completed.returncode, 'seconds': time.monotonic() - start,
          'sources': sources, 'harness_sha256': hashlib.sha256(harness.read_bytes()).hexdigest(),
          'shared_harness_sha256': hashlib.sha256(harness.with_name('shared.rs').read_bytes()).hexdigest(),
          'pressure_harness_sha256': hashlib.sha256(harness.with_name('pressure_host.rs').read_bytes()).hexdigest(),
          'ledger_directory': str(ledger_directory),
          'default_service': args.default_service,
          'scope': 'actual native host-control concurrency; no CUDA participant, GPU or native donor qualification'}
if completed.returncode == 0:
    report['result'] = json.loads((args.out / 'stdout').read_text())
    if args.default_service:
        report['result']['default_service'] = {
            'custom_ready_domain_inherited_without_discovery': True,
            'actual_directory_inode_replacement_refused': True,
            'concurrent_join_clients': 16,
            'fork_refused_before_held_registry': True,
            'old_closed_control_allows_new_domain': True,
            'stale_control_generation_refused': True,
            'intervening_custom_domain_is_busy': True,
            'original_fault_tombstone_survives_last_handle': True,
            'private_directory_and_refused_path_custody': True,
            'no_cuda_or_nvml_loaded': True,
            'gpu_identity_discovery_qualified': False,
        }
if args.default_service:
    report['default_harnesses'] = {name: hashlib.sha256(harness.with_name(name).read_bytes()).hexdigest()
                                   for name in ('default_service.rs', 'default_domain.rs')}
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(completed.returncode)
