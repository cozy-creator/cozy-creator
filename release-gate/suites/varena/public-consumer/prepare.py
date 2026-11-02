"""Freeze exact wheels and matching native clients into an ordinary Cozy fixture."""

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


def elf(path):
    header = subprocess.check_output(["readelf", "-h", str(path)], text=True)
    dynamic = subprocess.check_output(["readelf", "-d", str(path)], text=True)
    versions = subprocess.check_output(
        ["readelf", "--version-info", str(path)], text=True
    )
    assert "ELF64" in header and "Advanced Micro Devices X86-64" in header
    needed = re.findall(r"Shared library: \[([^]]+)\]", dynamic)
    glibc = sorted(
        {
            tuple(map(int, value))
            for value in re.findall(r"GLIBC_(\d+)\.(\d+)", versions)
        }
    )
    assert not glibc or glibc[-1] <= (2, 28)
    assert "GLIBC_PRIVATE" not in versions and "GLIBC_ABI_" not in versions
    assert "libvarena.so" in needed
    assert not any(
        "python" in name.lower() or "torch" in name.lower() for name in needed
    )
    return {"needed": needed, "glibc": glibc}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--runtime-wheel", type=Path, required=True)
    parser.add_argument("--tensorfs-wheel", type=Path, required=True)
    parser.add_argument(
        "--native-build-receipt",
        type=Path,
        required=True,
        help="actual build receipt with source_sha, core_sha256, std_name/std_sha256 and binary file hashes",
    )
    parser.add_argument("--probe-lib", type=Path, required=True)
    parser.add_argument("--mixed-c", type=Path, required=True)
    parser.add_argument("--mixed-rust", type=Path, required=True)
    parser.add_argument("--source-helper", type=Path, required=True)
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
    ), "freeze source first"
    built = json.loads(args.native_build_receipt.read_text())
    assert re.fullmatch("[0-9a-f]{40}", built["source_sha"])
    payload = {"source_sha": args.source_sha, "gpu_uuid": args.gpu_uuid}
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
        assert all(
            payload[name] == built[name]
            for name in ("core_sha256", "std_name", "std_sha256")
        ), "helpers and installed private core/std must be one build cohort"
        tfs_names = (
            "varena/__init__.py",
            "varena/api.py",
            "varena/_runtime.py",
            "varena/_cooperative.py",
            "varena/torch.py",
            "_native.py",
            "_native_identity.py",
        )
        payload["tensorfs_files"] = {
            name: data_sha(wheel.read("tensorfs/" + name)) for name in tfs_names
        }
    with zipfile.ZipFile(args.runtime_wheel) as wheel:
        payload["runtime_version"] = wheel_version(wheel)
        runtime_names = (
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
            name: data_sha(wheel.read("cozy_runtime/" + name)) for name in runtime_names
        }
    fixture = args.source_root / "release-gate/suites/varena/public-consumer"
    project = args.output / "project"
    shutil.copytree(
        fixture / "controller",
        project,
        ignore=shutil.ignore_patterns("__pycache__", ".venv"),
    )
    binaries = project / "ci_varena_public/bin"
    (binaries / "lib").mkdir(parents=True)
    for name, data in libraries.items():
        (binaries / "lib" / name).write_bytes(data)
    files = {
        "public-probe.so": args.probe_lib,
        "mixed-c.so": args.mixed_c,
        "mixed-rust.so": args.mixed_rust,
        "source-c": args.source_helper,
    }
    hashes, elf_facts = {}, {}
    for name, path in files.items():
        assert sha(path) == built["binaries"][name], (
            name,
            "native build receipt differs",
        )
        target = binaries / name
        shutil.copy2(path, target)
        subprocess.run(
            ["patchelf", "--set-rpath", "$ORIGIN/lib", str(target)], check=True
        )
        if name == "source-c":
            target.chmod(0o755)
        hashes[name] = sha(target)
        elf_facts[name] = elf(target)
    for name, key in (
        ("public-probe.so", "helper_sha256"),
        ("mixed-c.so", "mixed_c_sha256"),
        ("mixed-rust.so", "mixed_rust_sha256"),
        ("source-c", "source_helper_sha256"),
    ):
        payload[key] = hashes[name]
    original_fixture = args.source_root / "release-gate/suites/varena/public-api"
    python_helpers = {}
    for name in ("streams.py", "mixed.py"):
        shutil.copy2(original_fixture / name, binaries / name)
        python_helpers[name] = sha(binaries / name)
    source_helper_path = "release-gate/suites/varena/public-source/source.c"
    source_helper = subprocess.check_output(
        ["git", "show", built["source_sha"] + ":" + source_helper_path],
        cwd=args.source_root,
    )
    manifest = {
        "source_sha": args.source_sha,
        "native_build": built,
        "native_distribution": native_distribution,
        "native_build_receipt_sha256": sha(args.native_build_receipt),
        "binaries": hashes,
        "elf": elf_facts,
        "python_helpers": python_helpers,
        "retirement_successor": json.loads(
            (fixture / "retirement-origin.json").read_text()
        ),
        "public_source_helper_source_sha256": data_sha(source_helper),
        "wheels": {
            "runtime": {
                "path": str(args.runtime_wheel),
                "sha256": sha(args.runtime_wheel),
            },
            "tensorfs": {
                "path": str(args.tensorfs_wheel),
                "sha256": sha(args.tensorfs_wheel),
            },
        },
        "scope": "actual public API and passive Working custody; GPU execution pending",
    }
    (binaries / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / "input.json").write_text(json.dumps(payload, indent=2) + "\n")
    tree = {
        str(path.relative_to(project)): sha(path)
        for path in sorted(project.rglob("*"))
        if path.is_file()
    }
    result = {
        "project": str(project),
        "source_sha": args.source_sha,
        "manifest": manifest,
        "input": payload,
        "files": tree,
        "gpu_uuid_bound": args.gpu_uuid != "UNBOUND",
        "gpu_executed": False,
        "entrypoints": [
            "public_basic",
            "passive_custody",
            "mixed_retirement",
            "mixed_native_first",
            "mixed_python_first",
            "source_ordering",
            "source_partial",
            "source_prepare_error",
            "source_pending",
            "source_retained",
            "host_bytes",
        ],
    }
    (args.output / "ready.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
