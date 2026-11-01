"""Freeze the actual fixture and derive strict input hashes from paired wheel bytes."""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path

RUNTIME = (
    'internal/residency.py', 'internal/execution/phase.py',
    'internal/execution/committed_linear.py', 'internal/execution/catch.py',
        'internal/int8_linear.py', 'internal/prebuilt.py', 'lora.py',
    'internal/execution/native_split.py', 'internal/execution/native_linear_split.py',
    'internal/execution/native_residual_ffn.py', 'internal/encoding/leaves.py',
    'author/kernels.py', 'author/_split.py', '_kernels/_gemm.abi3.so',
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def wheel(path):
    with zipfile.ZipFile(path) as archive:
        files = {name: archive.read(name) for name in archive.namelist()}
    metadata = next(value.decode() for name, value in files.items() if name.endswith('.dist-info/METADATA'))
    version = next(line.removeprefix('Version: ') for line in metadata.splitlines() if line.startswith('Version: '))
    return version, files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime-wheel', type=Path, required=True)
    parser.add_argument('--tensorfs-wheel', type=Path, required=True)
    parser.add_argument('--gpu-uuid', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    assert not args.output.exists(), 'preserve prior prepared artifacts'
    commit = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    assert not subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain', '--', '.'])
    rv, runtime = wheel(args.runtime_wheel)
    tv, tensorfs = wheel(args.tensorfs_wheel)
    assert b'commit_execution' in runtime['cozy_runtime/internal/execution/committed_linear.py']
    assert b'plan_generation' in runtime['cozy_runtime/internal/execution/phase.py']
    assert b'supports_phase_plans' in tensorfs['tensorfs/varena/__init__.py']
    assert b'commit_execution' in tensorfs['tensorfs/varena/torch.py']
    extension = next(name for name in tensorfs if name.startswith('tensorfs/_ext.') and name.endswith('.so'))
    payload = {'runtime_version': rv,
               'runtime_modules': {name: digest(runtime['cozy_runtime/' + name]) for name in RUNTIME},
               'tensorfs_version': tv, 'extension_sha256': digest(tensorfs[extension]),
               'shim_sha256': digest(tensorfs['tensorfs/_varena_torch.so']),
               'tensorfs_torch_sha256': digest(tensorfs['tensorfs/varena/torch.py']),
               'gpu_uuid': args.gpu_uuid}
    target = args.output / 'project'
    shutil.copytree(source, target, ignore=shutil.ignore_patterns('__pycache__', '.venv', '.task'))
    (args.output / 'input.json').write_text(json.dumps(payload, indent=2) + '\n')
    provenance = {'fixture_commit': commit, 'runtime_wheel': str(args.runtime_wheel),
                  'runtime_wheel_sha256': digest(args.runtime_wheel.read_bytes()),
                  'tensorfs_wheel': str(args.tensorfs_wheel),
                  'tensorfs_wheel_sha256': digest(args.tensorfs_wheel.read_bytes()),
                  'files': {str(p.relative_to(target)): digest(p.read_bytes())
                            for p in sorted(target.rglob('*')) if p.is_file()},
                  'scope': 'source/package preparation only; root performs GPU execution'}
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps({'project': str(target), 'input': str(args.output / 'input.json'),
                      'fixture_commit': commit, 'files': len(provenance['files'])}))


if __name__ == '__main__':
    main()
