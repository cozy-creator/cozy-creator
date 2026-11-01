"""Archive a clean, exact Cache source checkpoint for the build operator."""
import argparse
import hashlib
import json
import subprocess
import tarfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()

    def git(*values):
        return subprocess.check_output(['git', '-C', str(args.repository), *values])

    assert not git('status', '--porcelain'), 'freeze a clean task checkpoint; preserve all WIP'
    source_sha = git('rev-parse', 'HEAD').decode().strip()
    args.output.mkdir(parents=True, exist_ok=False)
    archive = args.output / 'source.tar'
    with archive.open('wb') as file:
        subprocess.run(['git', '-C', str(args.repository), 'archive', source_sha], stdout=file, check=True)
    tree = args.output / 'source'
    tree.mkdir()
    with tarfile.open(archive) as tar:
        tar.extractall(tree, filter='data')
    files = {}
    for name in git('ls-tree', '-r', '--name-only', source_sha).decode().splitlines():
        path = tree / name
        data = path.read_bytes()
        assert data == git('show', f'{source_sha}:{name}'), name
        files[name] = hashlib.sha256(data).hexdigest()
    receipt = {'source_sha': source_sha, 'helper_source': source_sha, 'native_source': source_sha,
               'native_base': 'c3bf579ae82b42b254aa30fe54c609fc8e89afcb',
               'native_tree': git('rev-parse', f'{source_sha}:crates/varena').decode().strip(),
               'archive': str(archive.resolve()), 'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
               'source_directory': str(tree.resolve()), 'files': files,
               'scope': 'native Cache pages, finite asynchronous owner recovery, weak Python callbacks and real native/Runtime owner fixtures; GPU qualification is checkpoint-specific'}
    (args.output / 'source.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'source_sha': source_sha, 'files': len(files), 'source_directory': str(tree)}))


if __name__ == '__main__':
    main()
