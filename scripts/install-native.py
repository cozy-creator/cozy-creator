#!/usr/bin/env python3
"""Install an already verified dev bundle in a chosen prefix (CI/local proof)."""

import argparse
import hashlib
import json
import shutil
import tarfile
import tempfile
import zipfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("archive", type=Path)
parser.add_argument("directory", type=Path)
args = parser.parse_args()
args.directory.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(
    prefix="native-install-", dir=args.directory
) as temporary:
    root = Path(temporary)
    if args.archive.suffix == ".zip":
        with zipfile.ZipFile(args.archive) as archive:
            archive.extractall(root)
    else:
        with tarfile.open(args.archive) as archive:
            archive.extractall(root, filter="data")
    (manifest,) = root.rglob("varena-native.json")
    description = json.loads(manifest.read_text())
    for library in description["libraries"]:
        name = library["name"]
        if Path(name).name != name:
            raise ValueError("invalid native filename")
        source = manifest.parent / name
        if hashlib.sha256(source.read_bytes()).hexdigest() != library["sha256"]:
            raise ValueError("native dependency digest mismatch")
        destination = args.directory / name
        if destination.exists():
            if (
                hashlib.sha256(destination.read_bytes()).hexdigest()
                != library["sha256"]
            ):
                raise ValueError("different immutable native dependency")
        else:
            shutil.copy2(source, destination)
    name = description["binary"]
    if Path(name).name != name:
        raise ValueError("invalid binary filename")
    staged = args.directory / (name + ".new")
    shutil.copy2(manifest.parent / name, staged)
    staged.chmod(0o755)
    staged.replace(args.directory / name)
