"""Build exact old/new native ledger modules as real-process CPU release-gate binaries."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--old', default='c3bf579ae82b42b254aa30fe54c609fc8e89afcb')
parser.add_argument('--target', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
fixture = Path(__file__).resolve().parent
workspace = tomllib.loads((args.source / 'Cargo.toml').read_text())['workspace']


def toml(value):
    if isinstance(value, dict):
        return '{' + ', '.join(f'{key} = {toml(item)}' for key, item in value.items()) + '}'
    return json.dumps(value)


report = {'old_commit': args.old, 'gpu_execution': False, 'builds': {}}
for name, old in [('v2', True), ('v3', False)]:
    directory = args.out / name
    (directory / 'src/ledger').mkdir(parents=True)
    paths = ['crates/varena/src/ledger.rs', 'crates/varena/src/ledger/os.rs']
    if not old:
        paths.append('crates/varena/src/ledger/upgrade.rs')
    hashes = {}
    for path in paths:
        data = (subprocess.check_output(['git', 'show', f'{args.old}:{path}'], cwd=args.source)
                if old else (args.source / path).read_bytes())
        (directory / 'src' / Path(path).relative_to('crates/varena/src')).write_bytes(data)
        hashes[path] = hashlib.sha256(data).hexdigest()
    if not old:
        with (directory / 'src/ledger.rs').open('a') as output:
            output.write('\n' + (fixture / 'upgrade_gate.rs').read_text())
        with (directory / 'src/ledger/upgrade.rs').open('a') as output:
            output.write('\n' + (fixture / 'rename_only_control.rs').read_text())
    (directory / 'src/lib.rs').write_text('pub mod ledger;\n')
    shutil.copy2(fixture / 'peer.rs', directory / 'src/main.rs')
    text = ['[package]', f'name = "ledger-gate-{name}"', 'version = "0.1.0"',
            'edition = "2024"', '[lib]', 'name = "ledger_gate"', '[features]',
            f'default = {json.dumps([] if old else ["v3"])}', 'v3 = []', '[dependencies]']
    for dependency in ('libc', 'thiserror', 'serde_json'):
        text.append(f'{dependency} = {toml(workspace["dependencies"][dependency])}')
    (directory / 'Cargo.toml').write_text('\n'.join(text) + '\n')
    command = ['nice', '-n', '19', 'cargo', 'build', '--offline', '--manifest-path',
               str(directory / 'Cargo.toml'), '--target-dir', str(args.target), '-j', '1']
    with (directory / 'build.log').open('w') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
    assert result.returncode == 0, str(directory / 'build.log')
    binary = args.target / 'debug' / f'ledger-gate-{name}'
    copied = directory / binary.name
    shutil.copy2(binary, copied)
    report['builds'][name] = {'sources': hashes,
                            'gate_sha256': hashlib.sha256((fixture / 'upgrade_gate.rs').read_bytes()).hexdigest(),
                            'rename_only_control_sha256': hashlib.sha256((fixture / 'rename_only_control.rs').read_bytes()).hexdigest(),
                            'binary': str(copied.resolve()),
                            'binary_sha256': hashlib.sha256(copied.read_bytes()).hexdigest(), 'command': command}
(args.out / 'builds.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
