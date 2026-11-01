"""Freeze exact package modules beside this fixture; build input from actual wheel bytes."""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path

MODULES = (
    'author/kernels.py', 'internal/triton_kernels.py', 'internal/attention.py',
    'internal/kernel_sources.py', 'internal/kernel_ready.py',
)
PACKAGE = ('__init__.py', 'glue.py', 'adaln_pruned.py', 'overlay.py', 'producer_kernels.py')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def wheel(path):
    with zipfile.ZipFile(path) as archive:
        contents = {name: archive.read(name) for name in archive.namelist()}
    metadata = next(data.decode() for name, data in contents.items()
                    if name.endswith('.dist-info/METADATA'))
    version = next(line.removeprefix('Version: ') for line in metadata.splitlines()
                   if line.startswith('Version: '))
    return version, contents


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package-root', type=Path, required=True)
    parser.add_argument('--runtime-wheel', type=Path, required=True)
    parser.add_argument('--tensorfs-wheel', type=Path, required=True)
    parser.add_argument('--kernel-receipt', type=Path, required=True)
    parser.add_argument('--gpu-uuid', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    root = args.package_root.resolve()
    assert not args.output.exists(), 'freeze into a new path; preserve previous receipts'
    git = ['git', '-C', str(root)]
    assert not subprocess.check_output(git + ['status', '--porcelain', '--untracked-files=no'])
    head = subprocess.check_output(git + ['rev-parse', 'HEAD'], text=True).strip()
    runtime_version, runtime = wheel(args.runtime_wheel)
    tensorfs_version, tensorfs = wheel(args.tensorfs_wheel)
    kernels = json.loads(args.kernel_receipt.read_text())
    assert kernels['cuda_initialized'] is False
    assert kernels['source_sha256'] == sha((root / 'minimax_h3/glue.py').read_bytes())
    project = args.output / 'project'
    shutil.copytree(source, project, ignore=shutil.ignore_patterns('__pycache__', '.venv', '.task'))
    package = project / 'minimax_h3'
    package.mkdir()
    package_hashes = {}
    for name in PACKAGE:
        data = (root / 'minimax_h3' / name).read_bytes()
        (package / name).write_bytes(data)
        package_hashes[name] = sha(data)
    extension = next(name for name in tensorfs if name.startswith('tensorfs/')
                     and name.endswith('.so') and '/_ext.' in name)
    payload = {
        'runtime_version': runtime_version,
        'runtime_modules': {name: sha(runtime['cozy_runtime/' + name]) for name in MODULES},
        'tensorfs_version': tensorfs_version,
        'extension_sha256': sha(tensorfs[extension]),
        'shim_sha256': sha(tensorfs['tensorfs/_varena_torch.so']),
        'gpu_uuid': args.gpu_uuid,
        'package_modules': package_hashes,
        'kernel_identities': {name: value['identity'] for name, value in kernels['sets'].items()},
    }
    (args.output / 'input.json').write_text(json.dumps(payload, indent=2) + '\n')
    provenance = {
        'package_head': head, 'package_root': str(root), 'package_modules': package_hashes,
        'kernel_receipt': str(args.kernel_receipt),
        'kernel_receipt_sha256': sha(args.kernel_receipt.read_bytes()),
        'runtime_wheel': str(args.runtime_wheel), 'tensorfs_wheel': str(args.tensorfs_wheel),
        'runtime_wheel_sha256': sha(args.runtime_wheel.read_bytes()),
        'tensorfs_wheel_sha256': sha(args.tensorfs_wheel.read_bytes()),
        'files': {str(p.relative_to(project)): sha(p.read_bytes())
                  for p in sorted(project.rglob('*')) if p.is_file()},
    }
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps({'project': str(project), 'input': str(args.output / 'input.json'),
                      'package_head': head, 'files': len(provenance['files'])}))


if __name__ == '__main__':
    main()
