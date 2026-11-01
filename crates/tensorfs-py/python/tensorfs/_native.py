"""Load the shared Varena core packaged with this distribution (Linux ABI 1)."""

from __future__ import annotations

import ctypes
import importlib.metadata
import os
import sys
from pathlib import Path

_libraries: list[ctypes.CDLL] = []


def load() -> None:
    if sys.platform != "linux":
        return
    # SDK bundles use .native; installed wheels use the prefix data directory so
    # the native tfs executable and Python resolve the same relocatable libraries.
    bundled = Path(__file__).parent / ".native"
    directory = bundled if bundled.is_dir() else Path(sys.prefix) / "lib" / "varena-v1"
    if not bundled.is_dir():
        try:
            distribution = importlib.metadata.distribution("tensorfs")
            for file in distribution.files or ():
                if file.as_posix().endswith("lib/varena-v1/libvarena.so"):
                    directory = Path(distribution.locate_file(file)).parent
                    break
        except importlib.metadata.PackageNotFoundError:
            pass  # A source checkout can use the ordinary loader search path.
    for path in [*sorted(directory.glob("libstd-*.so")), directory / "libvarena.so"]:
        try:
            library = ctypes.CDLL(path.name, mode=os.RTLD_NOLOAD | os.RTLD_GLOBAL)
        except OSError:
            # A source checkout may use the loader's normal library search path.
            library = ctypes.CDLL(
                str(path) if path.is_file() else path.name, mode=os.RTLD_GLOBAL
            )
        _libraries.append(library)
