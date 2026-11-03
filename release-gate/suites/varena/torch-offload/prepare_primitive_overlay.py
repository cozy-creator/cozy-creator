"""Freeze the unwired 594/006 primitive composition over existing native771.

Repackages existing binaries byte-for-byte. Never builds native code or installs
anything. Per-file origins distinguish this composition from a single Git tree.
"""

from __future__ import annotations

import argparse
import ast
import base64
import csv
import hashlib
import io
import json
import subprocess
import zipfile
from pathlib import Path

BASE = "eb3157bb1aa2aed79685f0a80cb2e0a193ee01a5"
PYTHON = "7eee17802b2877e318fcefccf8ee015dfa9c66be"
OPTIONAL = "594bfb49ba81d1397e1b3fa4a5b118310fe8f8b0"
CACHE = "006bdf850b6de5c7444ef60299cec187e4cc3659"
NATIVE = "77178571c6968da69af66e2a811c93101da6bf69"
PREFIX = "crates/tensorfs-py/python/"
VERSION = "0.17.16+dev.g77178571.primitives7eee1780"
WHEEL_SHA = "e5ffedbc29a65dd84643c62f57e06cbac0ecbf05f034bbec1804c7839e30fa23"
PROVIDER_SHA = "a0fe8451d38e5c089737b64d5dac57ae9a93cbe5a0ccc95334d8d794ba1a8ac1"
OVERRIDES = (
    "_offload.py",
    "_offload_auto.py",
    "_offload_graph.py",
    "_offload_stream.py",
    "_offload_sdxl.py",
    "_offload_review.py",
)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args])


def blob(repo, commit, path):
    return git(repo, "show", commit + ":" + path)


def class_ast(data, name):
    node = next(
        node
        for node in ast.parse(data).body
        if isinstance(node, ast.ClassDef) and node.name == name
    )
    return ast.dump(node, include_attributes=False)


def prepare(args):
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    assert sha(args.native_wheel.read_bytes()) == WHEEL_SHA
    assert sha(args.provider.read_bytes()) == PROVIDER_SHA
    paths = (
        git(args.repo, "ls-tree", "-r", "--name-only", BASE, PREFIX + "tensorfs")
        .decode()
        .splitlines()
    )
    files, origins = {}, {}

    def put(target, commit, source):
        data = blob(args.repo, commit, source)
        files[target] = data
        origins[target] = {
            "commit": commit,
            "path": source,
            "git_blob": git(args.repo, "rev-parse", commit + ":" + source)
            .decode()
            .strip(),
            "sha256": sha(data),
        }

    for path in paths:
        put(path.removeprefix(PREFIX), BASE, path)
    for name in OVERRIDES:
        path = "tensorfs/varena/" + name
        put(path, PYTHON, PREFIX + path)
    auto_path = PREFIX + "tensorfs/varena/_offload_auto.py"
    auto = files["tensorfs/varena/_offload_auto.py"]
    assert class_ast(auto, "_Preparation") == class_ast(
        blob(args.repo, OPTIONAL, auto_path), "_Preparation"
    )
    assert class_ast(auto, "_GraphCacheEntry") == class_ast(
        blob(args.repo, CACHE, auto_path), "_GraphCacheEntry"
    )
    assert b"class _AutomaticGraphs" not in auto
    assert b"self._automatic" not in files["tensorfs/varena/_offload.py"]
    graph_path = "tensorfs/varena/_offload_graph.py"
    assert files[graph_path] == blob(args.repo, CACHE, PREFIX + graph_path)
    for name, commit in (
        ("optional_pool_admission_gpu.py", OPTIONAL),
        ("automatic_cache_pressure_gpu.py", CACHE),
    ):
        put(
            "clients/" + name,
            commit,
            "release-gate/suites/varena/torch-offload/" + name,
        )
    for name in ("provider.py", "source_provider.c"):
        put(
            "helpers/" + name,
            NATIVE,
            "release-gate/suites/varena/native-tensor-export/" + name,
        )
    put(
        "provider-source/public-source/source.c",
        NATIVE,
        "release-gate/suites/varena/public-source/source.c",
    )
    put("provider-source/include/varena.h", NATIVE, "crates/varena/include/varena.h")
    files["helpers/varena-tensor-export-provider.so"] = args.provider.read_bytes()
    with zipfile.ZipFile(args.native_wheel) as archive:
        wheel = {
            name: archive.read(name)
            for name in archive.namelist()
            if not name.endswith("/")
        }
    natives = {
        name: sha(data)
        for name, data in wheel.items()
        if name.startswith("tensorfs/")
        and (name.endswith((".so", "_varena_torch.json")) or "/.native/" in name)
    }
    # Replace ALL package Python/stub sources; there is no partial module overlay.
    for name in list(wheel):
        if name.startswith("tensorfs/") and (
            name.endswith((".py", ".pyi", "py.typed"))
        ):
            del wheel[name]
    for name, data in files.items():
        if name.startswith("tensorfs/"):
            wheel[name] = data
    old_info = next(
        name.split("/")[0] for name in wheel if name.endswith(".dist-info/METADATA")
    )
    new_info = "tensorfs-" + VERSION + ".dist-info"
    for name in list(wheel):
        if name.startswith(old_info + "/"):
            wheel[new_info + name[len(old_info) :]] = wheel.pop(name)
    metadata = wheel[new_info + "/METADATA"].decode()
    metadata = "\n".join(
        "Version: " + VERSION if line.startswith("Version: ") else line
        for line in metadata.split("\n")
    )
    wheel[new_info + "/METADATA"] = metadata.encode()
    record_path = new_info + "/RECORD"
    wheel.pop(record_path, None)
    record = io.StringIO()
    writer = csv.writer(record, lineterminator="\n")
    for name, data in sorted(wheel.items()):
        digest = (
            base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip("=")
        )
        writer.writerow((name, "sha256=" + digest, len(data)))
    writer.writerow((record_path, "", ""))
    wheel[record_path] = record.getvalue().encode()
    wheel_name = "tensorfs-" + VERSION + "-cp311-abi3-linux_x86_64.whl"
    with zipfile.ZipFile(
        out / wheel_name, "w", compression=zipfile.ZIP_DEFLATED
    ) as archive:
        for name, data in sorted(wheel.items()):
            info = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)
    with zipfile.ZipFile(out / wheel_name) as archive:
        assert all(
            sha(archive.read(name)) == digest for name, digest in natives.items()
        )
    for name, data in files.items():
        target = out / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    manifest = {
        "kind": "EXACT_COMPOSITION_NOT_A_SINGLE_GIT_TREE",
        "label": "eb315-complete-package+7eee-unwired-594-006-primitives",
        "package_base_commit": BASE,
        "six_python_overrides_commit": PYTHON,
        "controller_wired": False,
        "normal_offload_behavior": "eager; explicit owner.capture only",
        "mechanisms": {
            "_Preparation": OPTIONAL,
            "_GraphCacheEntry_and_private_retirement": CACHE,
        },
        "class_ast_checks": "Preparation identical to594; GraphCacheEntry identical to006; graph module identical to006",
        "file_origins": origins,
        "file_hashes": {name: sha(data) for name, data in sorted(files.items())},
        "prebuilt_provider": {
            "path": str(args.provider.resolve()),
            "sha256": PROVIDER_SHA,
            "fixture_abi": 1,
            "must_share_installed_core": True,
            "rebuilt": False,
        },
        "native": {
            "commit": NATIVE,
            "input_wheel": str(args.native_wheel.resolve()),
            "input_wheel_sha256": WHEEL_SHA,
            "files_preserved_byte_for_byte": natives,
            "native_build_performed": False,
        },
        "controlled_wheel": {
            "filename": wheel_name,
            "version": VERSION,
            "sha256": sha((out / wheel_name).read_bytes()),
            "scope": "controlled qualification repack, not a release",
        },
        "child_commands": [
            ["python", "-B", "clients/optional_pool_admission_gpu.py", "--device", "0"],
            [
                "python",
                "-B",
                "clients/automatic_cache_pressure_gpu.py",
                "--device",
                "0",
                "--provider",
                "helpers/varena-tensor-export-provider.so",
                "--provider-python",
                "helpers/provider.py",
                "--timeout-s",
                "60",
            ],
        ],
        "scope": "Ready source/native binding; GPU execution UNRUN here. Root owns guarded ordinary-Cozy runs. No machine/install/rental action performed.",
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(
        json.dumps(
            {
                "output": str(out),
                "manifest_sha256": sha((out / "manifest.json").read_bytes()),
                "wheel": manifest["controlled_wheel"],
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native-wheel", type=Path, required=True)
    parser.add_argument("--provider", type=Path, required=True)
    prepare(parser.parse_args())
