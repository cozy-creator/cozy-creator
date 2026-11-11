"""Record actual installed bytes and CPU settings for retained numerical controls."""

import hashlib
import importlib
from importlib import metadata
import json
import os
from pathlib import Path
import platform
import sys


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def package(name: str) -> dict[str, object]:
    module = importlib.import_module(name)
    origin = module.__file__
    if origin is None:
        raise ValueError(f"{name} has no installed module path")
    root = Path(origin).resolve().parent
    files = {f"{name}/{path.relative_to(root).as_posix()}": digest(path)
             for path in sorted(root.rglob("*"))
             if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc"}
    return {"path": str(root), "files": files}


def selected(value: str) -> dict[str, str]:
    path = Path(value)
    return {"selector": value, "wheel_sha256": digest(path)} if path.is_file() else {"selector": value}


def main() -> None:
    out, runtime, tensorfs, fixture = sys.argv[1:]
    torch = importlib.import_module("torch")
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                if line.startswith("model name")), "unknown")
    versions: dict[str, str | None] = {}
    for name in ("cozy-runtime", "tensorfs", "torch", "diffusers", "numpy"):
        try:
            versions[name] = metadata.version(name)
        except metadata.PackageNotFoundError:
            versions[name] = None
    value = {
        "python": sys.executable, "cpu_model": cpu, "machine": platform.machine(), "versions": versions,
        "selected_runtime": selected(runtime), "selected_tensorfs": selected(tensorfs),
        "modules": {name: package(name) for name in ("cozy_runtime", "tensorfs")},
        "fixture": {"path": fixture, "sha256": digest(Path(fixture))},
        "threads": {"torch": torch.get_num_threads(), "interop": torch.get_num_interop_threads(),
                    "float32_matmul_precision": torch.get_float32_matmul_precision(),
                    "environment": {name: os.environ.get(name) for name in
                                    ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS")}},
    }
    Path(out).write_text(json.dumps(value, indent=2) + "\n")


if __name__ == "__main__":
    main()
