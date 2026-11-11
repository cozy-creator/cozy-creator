#!/usr/bin/env python3
"""Stage the pinned filesystem helper for embedding; no runtime download is needed."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--out-dir', type=Path, required=True)
    parser.add_argument('--github-env', type=Path)
    args = parser.parse_args()
    if args.target.endswith('apple-darwin'):
        if args.github_env:
            with args.github_env.open('a', encoding='utf-8') as output:
                output.write('COZY_VAULT_NATIVE_APFS=1\n')
        print('/usr/bin/hdiutil')
        return
    repo = Path(__file__).resolve().parent.parent
    manifest = json.loads((repo / 'crates/project-mount/helper/manifest.json').read_text())
    try:
        pin = manifest['targets'][args.target]
    except KeyError:
        parser.error(f'no securefs helper is pinned for {args.target}')
    destination = args.out_dir.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / pin['asset']
    if not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != pin['sha256']:
        url = f"https://github.com/{manifest['repository']}/releases/download/v{manifest['version']}/{pin['asset']}"
        request = urllib.request.Request(url, headers={'User-Agent': 'cozy-project-vault-build'})
        with urllib.request.urlopen(request, timeout=120) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != pin['sha256']:
            raise SystemExit('securefs archive checksum mismatch')
        with tempfile.NamedTemporaryFile(dir=destination, prefix='.securefs-', delete=False) as staged:
            staged.write(data)
            staged.flush()
            os.fsync(staged.fileno())
            temporary = Path(staged.name)
        temporary.replace(archive)
    member = 'securefs.exe' if 'windows' in args.target else 'securefs'
    with zipfile.ZipFile(archive) as source:
        if source.getinfo(member).file_size <= 0:
            raise SystemExit('the pinned helper archive has no executable')
    if args.github_env:
        with args.github_env.open('a', encoding='utf-8') as output:
            output.write(f'COZY_SECUREFS_ARCHIVE={archive}\nCOZY_SECUREFS_REQUIRED=1\n')
    print(archive)


if __name__ == '__main__':
    main()
