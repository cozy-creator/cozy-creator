#!/usr/bin/env python3
"""Build the optional P0 Torch shim ahead of wheel assembly, without CUDA initialization."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess

import torch


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path(__file__).parents[1] / "python/tensorfs")
    args = parser.parse_args()
    if platform.system() != "Linux":
        raise SystemExit("the P0 Torch shim build currently qualifies Linux only")
    root = Path(torch.__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=True)
    output = args.out / "_varena_torch.so"
    temporary = output.with_suffix(".so.building")
    abi = int(torch._C._GLIBCXX_USE_CXX11_ABI)
    try:
        subprocess.run([
            os.environ.get("CXX", "c++"), "-O2", "-std=c++17", "-shared", "-fPIC",
            "-Wall", "-Wextra", "-Werror", f"-D_GLIBCXX_USE_CXX11_ABI={abi}",
            f"-I{root / 'include'}", str(Path(__file__).with_name("varena_torch.cpp")),
            f"-L{root / 'lib'}", "-lc10", "-Wl,--no-undefined",
            "-Wl,-rpath,$ORIGIN/../torch/lib", "-o", str(temporary),
        ], check=True)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    metadata = {
        "abi": 1, "torch": torch.__version__, "cxx11_abi": abi,
        "system": platform.system(), "machine": platform.machine(),
        "torch_git": torch.version.git_version,
    }
    output.with_suffix(".json").write_text(json.dumps(metadata, sort_keys=True) + "\n")
    print(f"built {output} against Torch {torch.__version__}; no CUDA context initialized")


if __name__ == "__main__":
    main()
