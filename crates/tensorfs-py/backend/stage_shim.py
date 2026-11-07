"""Stage one Torch allocator shim per Torch the machines run, before wheel construction."""
from __future__ import annotations

import platform
import shutil
import subprocess
from pathlib import Path

# Every Torch our worker images and machines run, as its exact torch.__version__ (PyPI's Linux
# build of each). A Torch outside this list is refused at install, naming it.
TORCH = ("2.15.0+cu132", "2.14.0+cu130")


def stage(root: Path, cargo_command: list[str], environment: dict[str, str]) -> None:
    target = environment.get("CARGO_BUILD_TARGET", "")
    if "--target" in cargo_command:
        target = cargo_command[cargo_command.index("--target") + 1]
    if platform.system() != "Linux" or target not in ("", f"{platform.machine()}-unknown-linux-gnu"):
        return  # Shims build natively on Linux; other wheels refuse varena's allocator.
    uv = shutil.which("uv")
    if uv is None:
        raise RuntimeError("the Linux wheel builds its Torch shims with uv")
    out = root / "python/tensorfs"
    for old in out.glob("_varena_torch-*.so"):
        old.unlink()
    offline = ["--offline"] if "--offline" in cargo_command or "--frozen" in cargo_command else []
    for torch in TORCH:
        # cuda-toolkit takes the version this Torch pins, so these headers match its CUDA.
        subprocess.run([uv, "run", "--no-project", "--python", "3.12", "--with", f"torch=={torch.split('+')[0]}",
                        "--with", "cuda-toolkit[crt,cccl]", *offline, "python", str(root / "shim/build.py"), "--torch", torch],
                       cwd=root, env=environment, check=True)
