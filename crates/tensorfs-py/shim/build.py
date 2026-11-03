#!/usr/bin/env python3
"""Build the Linux Torch allocator shim ahead of wheel assembly, without CUDA initialization."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess

import torch

TORCH_ABI_COHORT = "2.15.0+cu132"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path(__file__).parents[1] / "python/tensorfs")
    parser.add_argument("--cuda-crt-include", type=Path, help="matching CUDA CRT include directory, if staged separately")
    args = parser.parse_args()
    if platform.system() != "Linux":
        raise SystemExit("the Torch shim build currently qualifies Linux only")
    build_torch = str(torch.__version__)
    if build_torch != TORCH_ABI_COHORT or torch.version.cuda != "13.2":
        raise SystemExit(f"the reviewed shim ABI requires {TORCH_ABI_COHORT} with CUDA 13.2; got {build_torch} / {torch.version.cuda}")
    root = Path(torch.__file__).resolve().parent
    cuda_include = root.parent / "nvidia/cu13/include"
    cuda_crt_include = args.cuda_crt_include or cuda_include
    if not (cuda_include / "cuda_runtime_api.h").is_file():
        raise SystemExit("the current CUDA 13.2 runtime headers are required")
    if not (cuda_crt_include / "crt/host_defines.h").is_file():
        raise SystemExit("install matching nvidia-cuda-crt 13.2 headers for this shim build")
    args.out.mkdir(parents=True, exist_ok=True)
    output = args.out / "_varena_torch.so"
    temporary = output.with_suffix(".so.building")
    abi = int(torch._C._GLIBCXX_USE_CXX11_ABI)
    try:
        subprocess.run([
            os.environ.get("CXX", "c++"), "-O2", "-std=c++20", "-shared", "-fPIC",
            "-Wall", "-Wextra", "-Werror", "-DNDEBUG", f"-D_GLIBCXX_USE_CXX11_ABI={abi}",
            "-isystem", str(root / "include"), "-isystem", str(cuda_include), "-isystem", str(cuda_crt_include),
            str(Path(__file__).with_name("varena_torch.cpp")),
            f"-L{root / 'lib'}", f"-L{root.parent / 'nvidia/cu13/lib'}",
            "-ltorch_cuda", "-lc10_cuda", "-lc10", "-l:libcudart.so.13", "-Wl,--no-undefined",
            "-Wl,-rpath,$ORIGIN/../torch/lib:$ORIGIN/../nvidia/cu13/lib", "-o", str(temporary),
        ], check=True)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    metadata = {
        "abi": 2, "torch": build_torch, "supported_torch": [TORCH_ABI_COHORT],
        "cuda": torch.version.cuda, "cxx11_abi": abi,
        "system": platform.system(), "machine": platform.machine(),
        "torch_git": torch.version.git_version,
    }
    output.with_suffix(".json").write_text(json.dumps(metadata, sort_keys=True) + "\n")
    print(f"built {output} against Torch {torch.__version__}; no CUDA context initialized")


if __name__ == "__main__":
    main()
