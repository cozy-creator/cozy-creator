#!/usr/bin/env python3
"""Build the Linux Torch allocator shim against this interpreter's Torch, without CUDA initialization."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import platform
import subprocess

import torch


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path(__file__).parents[1] / "python/tensorfs")
    parser.add_argument("--torch", required=True, help="the exact torch.__version__ this build is for")
    args = parser.parse_args()
    if platform.system() != "Linux":
        raise SystemExit("the Torch shim builds on Linux only")
    if torch.__version__ != args.torch:
        raise SystemExit(f"the shim for Torch {args.torch} found Torch {torch.__version__}")
    root = Path(torch.__file__).resolve().parent
    cuda = root.parent / "nvidia/cu13"
    if not (cuda / "include/crt/host_defines.h").is_file():
        raise SystemExit(f"Torch {args.torch}'s CUDA headers need cuda-toolkit[crt,cccl]")
    args.out.mkdir(parents=True, exist_ok=True)
    output = args.out / f"_varena_torch-{args.torch}.so"
    temporary = output.with_suffix(".so.building")
    try:
        subprocess.run([
            os.environ.get("CXX", "c++"), "-O2", "-std=c++20", "-shared", "-fPIC",
            "-Wall", "-Wextra", "-Werror", "-DNDEBUG", f"-D_GLIBCXX_USE_CXX11_ABI={int(torch._C._GLIBCXX_USE_CXX11_ABI)}",
            "-isystem", str(root / "include"), "-isystem", str(cuda / "include"),
            str(Path(__file__).with_name("varena_torch.cpp")),
            f"-L{root / 'lib'}", f"-L{cuda / 'lib'}",
            "-ltorch_cuda", "-lc10_cuda", "-lc10", "-l:libcudart.so.13", "-Wl,--no-undefined",
            "-Wl,-rpath,$ORIGIN/../torch/lib:$ORIGIN/../nvidia/cu13/lib", "-o", str(temporary),
        ], check=True)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"built {output}; no CUDA context initialized")


if __name__ == "__main__":
    main()
