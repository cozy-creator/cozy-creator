#!/usr/bin/env python3
"""Bundle a Linux external SDK from the completed TensorFS dependency cohort.
Wheels now carry native dependencies directly through maturin/build.rs.

Run with uv run --no-project --with patchelf python crates/varena/bundle.py ...
No builds, installs or GPU access are performed by this file.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target",
        type=Path,
        required=True,
        help="exact Cargo target/profile directory",
    )
    parser.add_argument("--sdk", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("shared-core bundling is implemented for Linux only")
    sdk(args.target.resolve(), args.sdk.resolve())


if __name__ == "__main__":
    main()
