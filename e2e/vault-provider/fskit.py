#!/usr/bin/env python3
"""Disposable CI user setup, never called by Cozy: enable only signed macFUSE modules."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import subprocess

MODULES = {'io.macfuse.app.fsmodule.macfuse', 'io.macfuse.app.fsmodule.macfuse-local'}


def atomic(path, content):
    temporary = path.with_name('.cozy-ci-' + path.name)
    with temporary.open('wb') as output:
        os.chmod(temporary, 0o600)
        output.write(content)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['enable', 'restore'])
    parser.add_argument('--backup-dir', type=Path, required=True)
    args = parser.parse_args()
    if os.uname().sysname != 'Darwin':
        parser.error('FSKit CI setup is macOS-only')
    backup = args.backup_dir.resolve()
    state_file = backup / 'state.json'
    if args.operation == 'restore':
        if not state_file.exists():
            print(json.dumps({'restored': False, 'reason': 'setup did not modify settings'}))
            return
        state = json.loads(state_file.read_text())
        target = Path(state['target'])
        if state['existed']:
            atomic(target, (backup / 'original.plist').read_bytes())
        else:
            target.unlink(missing_ok=True)
        print(json.dumps({'restored': True, 'approved_modules': sorted(MODULES)}))
        return

    application = Path('/Library/Filesystems/macfuse.fs/Contents/Resources/macfuse.app')
    installed = set()
    for bundle in (application / 'Contents/Extensions').glob('*.appex'):
        with (bundle / 'Contents/Info.plist').open('rb') as source:
            identity = plistlib.load(source).get('CFBundleIdentifier')
        if identity in MODULES:
            subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
            installed.add(identity)
    if installed != MODULES:
        raise SystemExit('refusing FSKit setup: both signed macFUSE modules must be installed')
    target = Path.home() / 'Library/Group Containers/group.com.apple.fskit.settings/enabledModules.plist'
    existed = target.is_file()
    original = target.read_bytes() if existed else b''
    enabled = plistlib.loads(original) if existed else []
    if not isinstance(enabled, list) or not all(isinstance(value, str) for value in enabled):
        raise SystemExit('refusing to modify an unexpected FSKit module-list format')
    added = sorted(MODULES - set(enabled))
    backup.mkdir(parents=True, exist_ok=False, mode=0o700)
    if existed:
        (backup / 'original.plist').write_bytes(original)
    state_file.write_text(json.dumps({'target': str(target), 'existed': existed, 'added': added}) + '\n')
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    atomic(target, plistlib.dumps(enabled + added, fmt=plistlib.FMT_BINARY))
    print(json.dumps({'ci_only': True, 'signed_modules': sorted(installed), 'added': added, 'backup': str(backup)}))


if __name__ == '__main__':
    main()
