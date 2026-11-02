"""Stage the real consumer fixture from exact Runtime/native artifacts; no GPU action."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import zipfile
from pathlib import Path


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime-wheel', type=Path, required=True)
    parser.add_argument('--native-candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--gpu-uuid', default='UNBOUND')
    args = parser.parse_args()
    assert not args.output.exists()
    here = Path(__file__).resolve().parent
    repo = here.parents[3]
    assert not subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain'])
    fixture = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
    native = json.loads(args.native_candidate.read_text())
    assert sha(native['wheel_path']) == native['wheel_sha256']
    with zipfile.ZipFile(native['wheel_path']) as wheel:
        assert hashlib.sha256(wheel.read(native['extension_member'])).hexdigest() == native['extension_sha256']
    names = ('author/_loader.py', 'author/_model.py', 'internal/residency.py',
             'internal/cache_owners.py', 'internal/weights.py', 'internal/fill.py',
             'internal/resolution.py', 'internal/encoding/selection.py',
             'internal/int8_linear.py', 'internal/kernel_ready.py', 'internal/prebuilt.py')
    with zipfile.ZipFile(args.runtime_wheel) as wheel:
        metadata = [name for name in wheel.namelist() if name.endswith('.dist-info/METADATA')]
        assert len(metadata) == 1
        version = re.search(r'^Version: (.+)$', wheel.read(metadata[0]).decode(), re.MULTILINE)[1]
        modules = {name: hashlib.sha256(wheel.read('cozy_runtime/' + name)).hexdigest() for name in names}
    project = args.output / 'project'
    project.mkdir(parents=True)
    for name in ('pyproject.toml', 'uv.lock'):
        shutil.copyfile(here / name, project / name)
    shutil.copytree(here / 'ci_varena_cache_consumers', project / 'ci_varena_cache_consumers',
                    ignore=shutil.ignore_patterns('__pycache__'))
    payload = {'runtime_version': version, 'runtime_modules': modules,
               'tensorfs_version': native['tensorfs_version'], 'extension_sha256': native['extension_sha256'],
               'gpu_uuid': args.gpu_uuid}
    (args.output / 'input.json').write_text(json.dumps(payload, indent=2) + '\n')
    receipt = {'fixture': fixture, 'runtime_wheel': str(args.runtime_wheel),
               'runtime_wheel_sha256': sha(args.runtime_wheel), 'native_source': native['source_commit'],
               'native_wheel_sha256': native['wheel_sha256'], 'input': payload,
               'files': {str(p.relative_to(project)): sha(p) for p in sorted(project.rglob('*')) if p.is_file()},
               'gpu_executed': False}
    (args.output / 'ready.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'fixture': fixture, 'project': str(project), 'gpu_executed': False}))


if __name__ == '__main__':
    main()
