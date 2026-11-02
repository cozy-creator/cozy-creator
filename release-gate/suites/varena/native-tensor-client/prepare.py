"""Stage unchanged GPU clients and strict installed-wheel provenance; no GPU work."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import zipfile
from pathlib import Path


def sha(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def data_sha(data):
    return hashlib.sha256(data).hexdigest()


def wheel_version(wheel):
    metadata = [
        name for name in wheel.namelist() if name.endswith(".dist-info/METADATA")
    ]
    assert len(metadata) == 1
    found = re.search(
        r"^Version: (.+)$", wheel.read(metadata[0]).decode(), re.MULTILINE
    )
    assert found is not None
    return found[1]


def native_libraries(wheel):
    """Choose Python's canonical copies; every extra installed copy must match exactly."""
    core = [
        item for item in wheel.infolist() if item.filename.endswith("/libvarena.so")
    ]
    std = [
        item
        for item in wheel.infolist()
        if re.search(r"/libstd-[^/]+\.so$", item.filename)
    ]
    canonical_core = [
        item for item in core if item.filename == "tensorfs/.native/libvarena.so"
    ]
    canonical_std = [
        item
        for item in std
        if re.fullmatch(r"tensorfs/\.native/libstd-[^/]+\.so", item.filename)
    ]
    assert len(canonical_core) == len(canonical_std) == 1, (
        "one canonical Python core/std pair is required"
    )
    entries = core + std
    assert len({item.filename for item in entries}) == len(entries), (
        "duplicate wheel member names are ambiguous"
    )
    libraries = {
        "libvarena.so": wheel.read(canonical_core[0]),
        Path(canonical_std[0].filename).name: wheel.read(canonical_std[0]),
    }
    copies = {}
    for item in entries:
        name = Path(item.filename).name
        assert name in libraries, (
            item.filename,
            "a different Rust std cohort is distributed",
        )
        data = wheel.read(item)
        assert data == libraries[name], (
            item.filename,
            "distributed native copy differs from canonical Python bytes",
        )
        copies[item.filename] = data_sha(data)
    return libraries, {
        "canonical_core": canonical_core[0].filename,
        "canonical_std": canonical_std[0].filename,
        "distributed_copies": copies,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--script-source-sha", required=True)
    parser.add_argument("--tensorfs-wheel", type=Path, required=True)
    parser.add_argument("--runtime-wheel", type=Path, required=True)
    parser.add_argument("--gpu-uuid", default="UNBOUND")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    assert not args.output.exists(), "preserve earlier prepared artifacts"
    assert (
        subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=args.source_root, text=True
        ).strip()
        == args.source_sha
    )
    assert not subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=args.source_root, text=True
    ), "freeze first"
    assert re.fullmatch("[0-9a-f]{40}", args.script_source_sha)
    payload = {
        "source_sha": args.source_sha,
        "script_source_sha": args.script_source_sha,
        "gpu_uuid": args.gpu_uuid,
    }
    with zipfile.ZipFile(args.tensorfs_wheel) as wheel:
        payload["tensorfs_version"] = wheel_version(wheel)
        extension = [
            name
            for name in wheel.namelist()
            if re.fullmatch(r"tensorfs/_ext[^/]*\.so", name)
        ]
        assert len(extension) == 1
        payload["extension_sha256"] = data_sha(wheel.read(extension[0]))
        payload["shim_sha256"] = data_sha(wheel.read("tensorfs/_varena_torch.so"))
        libraries, native_distribution = native_libraries(wheel)
        payload["core_sha256"] = data_sha(libraries["libvarena.so"])
        payload["std_name"] = Path(native_distribution["canonical_std"]).name
        payload["std_sha256"] = data_sha(libraries[payload["std_name"]])
        files = [
            name
            for name in wheel.namelist()
            if name.startswith("tensorfs/varena/") and name.endswith(".py")
        ]
        files += [
            "tensorfs/__init__.py",
            "tensorfs/_native.py",
            "tensorfs/_native_identity.py",
        ]
        payload["tensorfs_files"] = {
            name.removeprefix("tensorfs/"): data_sha(wheel.read(name)) for name in files
        }
    with zipfile.ZipFile(args.runtime_wheel) as wheel:
        payload["runtime_version"] = wheel_version(wheel)
        names = (
            "internal/accel.py",
            "internal/cooperation.py",
            "internal/executor.py",
            "internal/residency.py",
            "internal/int8_linear.py",
            "internal/kernel_ready.py",
            "internal/prebuilt.py",
            "graphs/domain.py",
        )
        payload["runtime_files"] = {
            name: data_sha(wheel.read("cozy_runtime/" + name)) for name in names
        }
    fixture = args.source_root / "release-gate/suites/varena/native-tensor-client"
    project = args.output / "project"
    shutil.copytree(
        fixture / "controller",
        project,
        ignore=shutil.ignore_patterns("__pycache__", ".venv", ".ruff_cache"),
    )
    binaries = project / "ci_varena_tensor/bin"
    binaries.mkdir()
    payload["scripts"] = {}
    for name in ("gpu.py", "failed_tail_gpu.py"):
        rel = "release-gate/suites/varena/native-tensor-export/" + name
        original = subprocess.check_output(
            ["git", "show", args.script_source_sha + ":" + rel], cwd=args.source_root
        )
        assert original == (args.source_root / rel).read_bytes(), "script bytes changed"
        (binaries / name).write_bytes(original)
        payload["scripts"][name] = data_sha(original)
    manifest = {
        "source_sha": args.source_sha,
        "script_source_sha": args.script_source_sha,
        "scripts": payload["scripts"],
        "native_distribution": native_distribution,
        "wheels": {
            "tensorfs": {
                "path": str(args.tensorfs_wheel),
                "sha256": sha(args.tensorfs_wheel),
            },
            "runtime": {
                "path": str(args.runtime_wheel),
                "sha256": sha(args.runtime_wheel),
            },
        },
    }
    (binaries / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / "input.json").write_text(json.dumps(payload, indent=2) + "\n")
    ready = {
        "project": str(project),
        "input": payload,
        "manifest": manifest,
        "files": {
            str(p.relative_to(project)): sha(p)
            for p in sorted(project.rglob("*"))
            if p.is_file()
        },
        "entrypoints": ["typed_exports", "failed_tail"],
        "gpu_executed": False,
        "gpu_uuid_bound": args.gpu_uuid != "UNBOUND",
    }
    (args.output / "ready.json").write_text(json.dumps(ready, indent=2) + "\n")
    print(
        json.dumps(
            {
                key: ready[key]
                for key in ("project", "entrypoints", "gpu_executed", "gpu_uuid_bound")
            }
        )
    )


if __name__ == "__main__":
    main()
