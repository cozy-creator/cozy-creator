"""Verify the native Cache ELF/source and freeze an ordinary Cozy controller payload."""
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
    parser.add_argument('--source-receipt', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--native-extension-sha256', required=True)
    parser.add_argument('--gpu-uuid', default='UNBOUND')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert not args.output.exists(), 'preserve earlier prepared artifacts'
    assert re.fullmatch('[0-9a-f]{64}', args.native_extension_sha256)
    source = json.loads(args.source_receipt.read_text())
    assert sha(source['archive']) == source['archive_sha256']
    tree = Path(source['source_directory'])
    for name, expected in source['files'].items():
        assert sha(tree / name) == expected, name
    header = subprocess.check_output(['readelf', '-h', str(args.binary)], text=True)
    dynamic = subprocess.check_output(['readelf', '-d', str(args.binary)], text=True)
    versions = subprocess.check_output(['readelf', '--version-info', str(args.binary)], text=True)
    assert 'ELF64' in header and 'Advanced Micro Devices X86-64' in header
    needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
    assert needed and not any('python' in name.lower() or 'torch' in name.lower() for name in needed)
    glibc = sorted({tuple(map(int, item)) for item in re.findall(r'GLIBC_(\d+)\.(\d+)', versions)})
    assert glibc and glibc[-1] <= (2, 28)
    assert 'GLIBC_ABI_' not in versions and 'GLIBC_PRIVATE' not in versions
    manifest = {'source_sha': source['source_sha'], 'native_base': source['native_base'],
                'native_tree': source['native_tree'], 'binary_sha256': sha(args.binary),
                'archive_sha256': source['archive_sha256'], 'source_receipt_sha256': sha(args.source_receipt),
                'elf': {'class': 'ELF64', 'machine': 'x86_64', 'needed': needed,
                        'glibc_versions': glibc, 'glibc_max': glibc[-1]}, 'scope': source['scope']}
    project = args.output / 'project'
    shutil.copytree(tree / 'release-gate/suites/varena/cache-pages/controller', project,
                    ignore=shutil.ignore_patterns('__pycache__', '.venv'))
    target = project / 'ci_varena_cache_pages/bin'
    target.mkdir()
    shutil.copyfile(args.binary, target / 'varena-cache-pages')
    (target / 'varena-cache-pages').chmod(0o755)
    (target / 'provenance.json').write_text(json.dumps(manifest, indent=2) + '\n')
    payload = {'source_sha': source['source_sha'], 'helper_sha256': manifest['binary_sha256'],
               'native_extension_sha256': args.native_extension_sha256, 'gpu_uuid': args.gpu_uuid}
    (args.output / 'input-template.json').write_text(json.dumps(payload, indent=2) + '\n')
    files = {str(path.relative_to(project)): sha(path) for path in sorted(project.rglob('*')) if path.is_file()}
    receipt = {'manifest': manifest, 'project': str(project), 'files': files, 'input': payload,
               'gpu_uuid_bound': args.gpu_uuid != 'UNBOUND', 'gpu_executed': False}
    (args.output / 'ready.json').write_text(json.dumps(receipt, indent=2) + '\n')
    for name, text in [('elf-header', header), ('elf-dynamic', dynamic), ('elf-versions', versions)]:
        (args.output / f'{name}.txt').write_text(text)
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
