"""Stage the supported Linux x86_64 Torch adapter before wheel construction."""
from __future__ import annotations

import platform
import shutil
import subprocess
from pathlib import Path


def stage(root: Path, cargo_command: list[str], environment: dict[str, str]) -> None:
    target = environment.get("CARGO_BUILD_TARGET", "")
    if "--target" in cargo_command:
        target = cargo_command[cargo_command.index("--target") + 1]
    supported = target == "x86_64-unknown-linux-gnu" if target else (
        platform.system() == "Linux" and platform.machine() == "x86_64"
    )
    if not supported:
        return  # Existing CPU/non-x86_64 wheel routes do not claim this CUDA adapter.
    python = environment.get("TENSORFS_TORCH_PYTHON")
    if python:
        command = [python]
    else:
        uv = shutil.which("uv")
        if uv is None:
            raise RuntimeError("Linux x86_64 wheel staging needs uv or TENSORFS_TORCH_PYTHON with Torch2.15.0+cu132 and matching CUDA CRT13.2.86")
        command = [uv, "run", "--no-project", "--python", "3.12", "--with", "torch==2.15.0",
                   "--with", "nvidia-cuda-crt==13.2.86"]
        if "--offline" in cargo_command or "--frozen" in cargo_command:
            command.append("--offline")
        command.append("python")
    command.append(str(root / "shim/build.py"))
    crt = environment.get("TENSORFS_CUDA_CRT_INCLUDE")
    if crt:
        command.extend(["--cuda-crt-include", crt])
    subprocess.run(command, cwd=root, env=environment, check=True)
