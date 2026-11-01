#!/usr/bin/env python3
"""Bundle one Linux core for external SDK clients or an already built TensorFS wheel.

Run with uv run --no-project --with patchelf python crates/varena/bundle.py ...
No builds, installs or GPU access are performed by this file.
"""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import json
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def run(*args: str) -> str:
    return subprocess.check_output(args, text=True).strip()


def native_files(target: Path) -> list[Path]:
    sysroot = Path(run("rustc", "--print", "target-libdir"))
    # Cargo emits dependency dylibs with prefer-dynamic. A separately built primary
    # libvarena.so can have a different Rust ABI; always bundle the extension's dependency.
    files = [target / "deps" / "libvarena.so", *sysroot.glob("libstd-*.so")]
    if len(files) != 2 or not all(p.is_file() for p in files):
        raise RuntimeError(
            f"expected the built shared core and exactly one matching Rust std: {files}"
        )
    return files


def patch(path: Path, replacements: dict[str, str], rpath: str) -> None:
    for name in run("patchelf", "--print-needed", str(path)).splitlines():
        replacement = replacements.get(name)
        if replacement:
            run("patchelf", "--replace-needed", name, replacement, str(path))
    run("patchelf", "--set-rpath", rpath, str(path))


def sdk(target: Path, output: Path) -> None:
    if output.exists():
        raise RuntimeError("SDK output must be a new directory")
    package = output / "python" / "tensorfs"
    shutil.copytree(
        ROOT / "crates/tensorfs-py/python/tensorfs",
        package,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "_ext*.so", ".native"),
    )
    native = package / ".native"
    native.mkdir()
    for source in native_files(target):
        shutil.copy2(source, native / source.name)
        patch(native / source.name, {}, "$ORIGIN")
    shutil.copy2(target / "lib_ext.so", package / "_ext.so")
    patch(package / "_ext.so", {}, "$ORIGIN/.native")
    shutil.copy2(ROOT / "crates/varena/include/varena.h", output / "varena.h")
    print(json.dumps({"sdk": str(output), "native": str(native)}))


def wheel(target: Path, path: Path) -> None:
    # Preserve the wheel's platform/ABI tags and all unrelated repaired libraries.
    # auditwheel may rename core libraries; normalize only our two owned dependencies
    # so a simultaneously loaded C/Rust client finds the exact same SONAME.
    sources = native_files(target)
    with tempfile.TemporaryDirectory(prefix="varena-wheel-", dir=path.parent) as temp:
        tree = Path(temp)
        with zipfile.ZipFile(path) as archive:
            archive.extractall(tree)
            for item in archive.infolist():
                mode = (item.external_attr >> 16) & 0o777
                if mode and not item.is_dir():
                    (tree / item.filename).chmod(mode)
        metadata = list(tree.glob("*.dist-info"))
        if len(metadata) != 1:
            raise RuntimeError("expected one wheel distribution")
        data = tree / (metadata[0].name.removesuffix(".dist-info") + ".data")
        library = data / "data" / "lib" / "varena-v1"
        library.mkdir(parents=True, exist_ok=True)
        include = data / "data" / "include" / "varena" / "v1"
        include.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "crates/varena/include/varena.h", include / "varena.h")
        replacements = {}
        for candidate in list(tree.rglob("*.so")):
            if candidate.name.startswith(("libvarena", "libstd-")):
                replacement = (
                    "libvarena.so"
                    if candidate.name.startswith("libvarena")
                    else sources[1].name
                )
                replacements[candidate.name] = replacement
                candidate.unlink()
        for source in sources:
            shutil.copy2(source, library / source.name)
            patch(library / source.name, {}, "$ORIGIN")
        for extension in (tree / "tensorfs").glob("_ext*.so"):
            # _native.load preloads these by SONAME before _ext. Other repaired
            # dependencies keep their existing wheel-relative loader search paths.
            old_rpath = run("patchelf", "--print-rpath", str(extension))
            patch(extension, replacements, old_rpath)
        binary = data / "scripts" / "tfs"
        if not binary.is_file():
            raise RuntimeError(
                "the full TensorFS wheel must include its native tfs executable"
            )
        patch(binary, replacements, "$ORIGIN/../lib/varena-v1")
        record = metadata[0] / "RECORD"
        rows = []
        for file in sorted(tree.rglob("*")):
            if file.is_file() and file != record:
                blob = file.read_bytes()
                digest = (
                    base64.urlsafe_b64encode(hashlib.sha256(blob).digest())
                    .rstrip(b"=")
                    .decode()
                )
                rows.append(
                    [
                        file.relative_to(tree).as_posix(),
                        "sha256=" + digest,
                        str(len(blob)),
                    ]
                )
        rows.append([record.relative_to(tree).as_posix(), "", ""])
        content = io.StringIO(newline="")
        csv.writer(content, lineterminator="\n").writerows(rows)
        record.write_text(content.getvalue())
        replacement = path.with_suffix(".shared.whl")
        with zipfile.ZipFile(replacement, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in sorted(tree.rglob("*")):
                if file.is_file():
                    archive.write(file, file.relative_to(tree))
        replacement.replace(path)
    print(
        json.dumps(
            {
                "wheel": str(path),
                "core_sha256": hashlib.sha256(sources[0].read_bytes()).hexdigest(),
            }
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target",
        type=Path,
        required=True,
        help="exact Cargo target/profile directory",
    )
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--sdk", type=Path)
    group.add_argument("--wheel", type=Path)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("shared-core bundling is implemented for Linux only")
    (sdk if args.sdk else wheel)(
        args.target.resolve(), (args.sdk or args.wheel).resolve()
    )


if __name__ == "__main__":
    main()
