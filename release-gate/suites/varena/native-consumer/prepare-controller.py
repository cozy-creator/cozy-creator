"""Verify the portable native artifact and freeze its ordinary Cozy controller."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path


def sha(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--source-receipt', type=Path, required=True)
    parser.add_argument('--gpu-uuid', default='UNBOUND')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert not args.output.exists(), 'preserve earlier prepared artifacts'
    source = json.loads(args.source_receipt.read_text())
    archive = Path(source['archive'])
    assert sha(archive) == source['archive_sha256']
    tree = Path(source['source_directory'])
    for name, expected in source['files'].items():
        assert sha(tree / name) == expected, name
    header = subprocess.check_output(['readelf', '-h', str(args.binary)], text=True)
    dynamic = subprocess.check_output(['readelf', '-d', str(args.binary)], text=True)
    versions = subprocess.check_output(['readelf', '--version-info', str(args.binary)], text=True)
    assert 'ELF64' in header and 'Advanced Micro Devices X86-64' in header
    needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
    assert needed and not any('python' in name.lower() or 'torch' in name.lower() for name in needed)
    glibc = sorted({tuple(map(int, x)) for x in re.findall(r'GLIBC_(\d+)\.(\d+)', versions)})
    assert glibc and glibc[-1] <= (2, 28), glibc
    manifest = {'helper_source': source['helper_source'], 'native_source': source['native_source'],
                'binary_sha256': sha(args.binary), 'source_receipt_sha256': sha(args.source_receipt),
                'archive_sha256': source['archive_sha256'], 'native_tree': source['native_tree'],
                'elf': {'class': 'ELF64', 'machine': 'x86_64', 'needed': needed,
                        'glibc_versions': glibc, 'glibc_max': glibc[-1]},
                'qualification': 'source/build only; native dependency may be pre-withdrawal-fix'}
    controller = tree / 'release-gate/suites/varena/native-consumer/controller'
    project = args.output / 'project'
    shutil.copytree(controller, project, ignore=shutil.ignore_patterns('__pycache__', '.venv'))
    bin_dir = project / 'ci_varena_native_consumer/bin'
    bin_dir.mkdir()
    binary = bin_dir / 'varena-native-consumer'
    shutil.copyfile(args.binary, binary)
    binary.chmod(0o755)
    (bin_dir / 'provenance.json').write_text(json.dumps(manifest, indent=2) + '\n')
    payload = {key: manifest[key] for key in ('helper_source', 'native_source')}
    payload.update(helper_sha256=manifest['binary_sha256'], gpu_uuid=args.gpu_uuid)
    (args.output / 'input-template.json').write_text(json.dumps(payload, indent=2) + '\n')
    (args.output / 'elf-header.txt').write_text(header)
    (args.output / 'elf-dynamic.txt').write_text(dynamic)
    (args.output / 'elf-versions.txt').write_text(versions)
    files = {str(p.relative_to(project)): sha(p) for p in sorted(project.rglob('*')) if p.is_file()}
    receipt = {'manifest': manifest, 'project': str(project), 'files': files,
               'gpu_uuid_bound': args.gpu_uuid != 'UNBOUND', 'gpu_executed': False}
    (args.output / 'ready.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'project': str(project), 'files': len(files), 'manifest': manifest}, indent=2))


if __name__ == '__main__':
    main()
