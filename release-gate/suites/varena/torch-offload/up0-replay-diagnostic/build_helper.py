"""Build only the diagnostic helper against the installed exact Torch cohort."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

import torch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--cuda-crt-include", type=Path, required=True)
    args = parser.parse_args()
    assert torch.__version__ == "2.15.0+cu132"
    assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
    assert torch.version.cuda == "13.2" and not torch.version.debug
    assert not torch.cuda.is_initialized()
    here = Path(__file__).resolve().parent
    root = Path(torch.__file__).resolve().parent
    cuda = root.parent / "nvidia/cu13"
    assert (args.cuda_crt_include / "crt/host_defines.h").is_file()
    args.out.mkdir(parents=True, exist_ok=True)
    target = args.out / "allocation_trace.so"
    command = [
        os.environ.get("CXX", "c++"), "-O2", "-std=c++20", "-shared", "-fPIC",
        "-Wall", "-Wextra", "-Werror", "-DNDEBUG",
        f"-D_GLIBCXX_USE_CXX11_ABI={int(torch._C._GLIBCXX_USE_CXX11_ABI)}",
        "-isystem", str(root / "include"), "-isystem", str(cuda / "include"),
        "-isystem", str(args.cuda_crt_include), str(here / "allocation_trace.cpp"),
        f"-L{root / 'lib'}", f"-L{cuda / 'lib'}",
        "-ltorch_cuda", "-ltorch_cpu", "-lc10_cuda", "-lc10", "-l:libcudart.so.13",
        "-Wl,--no-undefined", "-o", str(target),
    ]
    result = subprocess.run(command, capture_output=True, text=True)
    (args.out / "compile.stdout").write_text(result.stdout)
    (args.out / "compile.stderr").write_text(result.stderr)
    sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    receipt = {
        "command": command, "returncode": result.returncode,
        "torch": torch.__version__, "torch_git": torch.version.git_version,
        "cuda_initialized": torch.cuda.is_initialized(),
        "cuda_driver_loaded": "libcuda.so" in Path("/proc/self/maps").read_text(),
        "source_sha256": sha(here / "allocation_trace.cpp"),
        "header_sha256": sha(here / "diagnostic_markers.h"),
        "helper_sha256": sha(target) if result.returncode == 0 else None,
        "native_core_built": False,
    }
    (args.out / "build.json").write_text(json.dumps(receipt, indent=2) + "\n")
    assert result.returncode == 0, result.stderr[-6000:]
    assert not receipt["cuda_initialized"] and not receipt["cuda_driver_loaded"]
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
