#!/usr/bin/env python3
"""Repair Linux wheels only after their private libraries have been packaged.

auditwheel must validate the actual ELF floor; it must not rename the already bundled
allocator. Original artifacts are retained outside the upload glob.
"""

import argparse
import subprocess
import tempfile
import zipfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
parser.add_argument("--platform", required=True)
args = parser.parse_args()
for wheel in list(args.directory.glob("*.whl")):
    with zipfile.ZipFile(wheel) as archive:
        has_shim = any(name.startswith("tensorfs/_varena_torch-") for name in archive.namelist())
    # Torch/CUDA belong to the explicitly matched external framework profile.
    # Never copy a second framework implementation into the TensorFS wheel.
    external = ["libtorch_cuda.so", "libc10_cuda.so", "libc10.so", "libcudart.so.13"] if has_shim else []
    excludes = [arg for name in external for arg in ("--exclude", name)]
    with tempfile.TemporaryDirectory(
        prefix="portable-", dir=args.directory
    ) as temporary:
        subprocess.run(
            [
                "auditwheel",
                "repair",
                *excludes,
                "--only-plat",
                "--plat",
                args.platform,
                "-w",
                temporary,
                str(wheel),
            ],
            check=True,
        )
        (repaired,) = Path(temporary).glob("*.whl")
        with zipfile.ZipFile(repaired) as archive:
            names = archive.namelist()
            if "tensorfs/.native/libvarena.so" not in names:
                raise RuntimeError(
                    "repair changed the canonical allocator library path"
                )
            (extension,) = (
                name
                for name in names
                if name.startswith("tensorfs/_ext") and name.endswith(".so")
            )
            path = Path(temporary) / "extension.so"
            path.write_bytes(archive.read(extension))
            needed = subprocess.check_output(
                ["patchelf", "--print-needed", str(path)], text=True
            ).splitlines()
            if "libvarena.so" not in needed:
                raise RuntimeError(
                    "repair renamed the process-wide allocator dependency"
                )
        original = args.directory / "unrepaired"
        original.mkdir(exist_ok=True)
        wheel.replace(original / wheel.name)
        repaired.replace(args.directory / repaired.name)
