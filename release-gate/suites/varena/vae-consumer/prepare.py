"""Freeze the genuine two-file producer and derive strict wheel identities."""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def wheel(path):
    with zipfile.ZipFile(path) as archive:
        contents = {name: archive.read(name) for name in archive.namelist()}
    metadata = next(data.decode() for name, data in contents.items() if name.endswith('.dist-info/METADATA'))
    version = next(line.removeprefix('Version: ') for line in metadata.splitlines() if line.startswith('Version: '))
    return version, contents


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package-root', type=Path, required=True)
    parser.add_argument('--kernel-receipt', type=Path, required=True)
    parser.add_argument('--runtime-wheel', type=Path, required=True)
    parser.add_argument('--tensorfs-wheel', type=Path, required=True)
    parser.add_argument('--gpu-uuid', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    package = args.package_root.resolve()
    assert not args.output.exists(), 'preserve earlier inputs'
    for root in (source, package):
        assert not subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain', '--untracked-files=no'])
    head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    package_head = subprocess.check_output(['git', '-C', str(package), 'rev-parse', 'HEAD'], text=True).strip()
    modules = next(ast.literal_eval(node.value) for node in ast.parse(
        (source / 'ci_varena_vae_consumer/__init__.py').read_text()).body if isinstance(node, ast.Assign)
        and any(isinstance(name, ast.Name) and name.id == 'MODULES' for name in node.targets))
    kernel = json.loads(args.kernel_receipt.read_text())
    assert kernel['passed'] and not kernel['cuda_initialized']
    assert kernel['source_sha256']['package_vae_tiles'] == sha((package / 'minimax_h3/vae_tiles.py').read_bytes())
    runtime_version, runtime = wheel(args.runtime_wheel)
    tensorfs_version, tensorfs = wheel(args.tensorfs_wheel)
    project = args.output / 'project'
    shutil.copytree(source, project, ignore=shutil.ignore_patterns('__pycache__', '.venv', '.task', '.ruff_cache'))
    destination = project / 'minimax_h3'
    destination.mkdir()
    package_hashes = {}
    for name in ('__init__.py', 'vae_tiles.py'):
        data = (package / 'minimax_h3' / name).read_bytes()
        (destination / name).write_bytes(data)
        package_hashes[name] = sha(data)
    extension = next(name for name in tensorfs if name.startswith('tensorfs/') and '/_ext.' in name and name.endswith('.so'))
    payload = {'runtime_version': runtime_version,
               'runtime_modules': {name: sha(runtime['cozy_runtime/' + name]) for name in modules},
               'tensorfs_version': tensorfs_version, 'extension_sha256': sha(tensorfs[extension]),
               'shim_sha256': sha(tensorfs['tensorfs/_varena_torch.so']), 'gpu_uuid': args.gpu_uuid,
               'package_modules': package_hashes, 'kernel_identity': kernel['kernel_identity']}
    (args.output / 'input.json').write_text(json.dumps(payload, indent=2) + '\n')
    provenance = {'fixture_head': head, 'package_head': package_head,
                  'scope': 'exact producer-only bounded fixture, not current full H3 package qualification',
                  'runtime_wheel': str(args.runtime_wheel), 'tensorfs_wheel': str(args.tensorfs_wheel),
                  'runtime_wheel_sha256': sha(args.runtime_wheel.read_bytes()),
                  'tensorfs_wheel_sha256': sha(args.tensorfs_wheel.read_bytes()),
                  'files': {str(p.relative_to(project)): sha(p.read_bytes()) for p in sorted(project.rglob('*')) if p.is_file()}}
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps({'project': str(project), 'input': str(args.output / 'input.json'), 'fixture_head': head}))


if __name__ == '__main__':
    main()
