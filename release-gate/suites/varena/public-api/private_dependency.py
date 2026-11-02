"""Host-only real-image mismatch proof; OLD_PACKAGE is a retained earlier SDK package."""

from __future__ import annotations

import argparse
import ctypes
import importlib.util
import json
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("old_package", type=Path)
    parser.add_argument(
        "--order", choices=["native-first", "python-first"], required=True
    )
    args = parser.parse_args()
    old = args.old_package.resolve()
    if args.order == "native-first":
        ctypes.CDLL(str(old / ".native/libvarena.so"), mode=ctypes.RTLD_GLOBAL)
        try:
            importlib.import_module("tensorfs")
        except ImportError as error:
            assert "private Rust dependency mismatch" in str(error), str(error)
            assert "tensorfs._ext" not in sys.modules
            message = str(error)
        else:
            raise AssertionError("mismatched preloaded core accepted")
    else:
        from tensorfs import _native

        # Exercise the new loader against the actual retained old extension/core
        # pair, after this installed extension has loaded its own core. No fake DSO.
        name = "tensorfs._old_dependency_check"
        spec = importlib.util.spec_from_file_location(name, _native.__file__)
        check = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(check)
        check.__file__ = str(old / "_native.py")
        try:
            check.load()
        except ImportError as error:
            assert "private Rust dependency mismatch" in str(error), str(error)
            message = str(error)
        else:
            raise AssertionError("second private dependency accepted")
    maps = Path("/proc/self/maps").read_text().splitlines()
    cores = {line.split()[-1] for line in maps if line.endswith("/libvarena.so")}
    assert len(cores) == 1, cores
    print(
        json.dumps(
            {
                "order": args.order,
                "private_mismatch_rejected": True,
                "mapped_cores": sorted(cores),
                "message": message,
                "gpu": False,
            }
        )
    )


if __name__ == "__main__":
    main()
