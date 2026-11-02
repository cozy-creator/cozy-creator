#!/usr/bin/env python3
"""Compare actual Cargo feature graphs; does not compile or touch a native target cache."""
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
from pathlib import Path


def tree(manifest: Path, packages: list[str], features: list[str], flags: list[str]) -> dict[str, list[str]]:
    command = ["cargo", "tree", "--locked", "--offline", "--manifest-path", str(manifest),
               "--edges", "normal,build", "--prefix", "none", "--format", "{p}|{f}"]
    for package in packages:
        command.extend(["-p", package])
    if features:
        command.extend(["--features", ",".join(features)])
    command.extend(flags)
    output = subprocess.check_output(command, text=True)
    result = {}
    for line in output.splitlines():
        if not line.strip():
            continue
        package, features = line.removesuffix(" (*)").split("|", 1)
        current = set(filter(None, features.split(",")))
        result[package] = sorted(current | set(result.get(package, [])))
    return result


def run() -> dict:
    parser = argparse.ArgumentParser()
    parser.add_argument("--interpreter", default=sys.executable)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[4]
    helper = repo / "crates/tensorfs-py/backend/stage_cli.py"
    manifest = repo / "crates/tensorfs-py/Cargo.toml"
    spec = importlib.util.spec_from_file_location("stage_cli", helper)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    version = json.loads(subprocess.check_output([args.interpreter, "-c",
        "import json,sys; print(json.dumps(list(sys.version_info[:2])))"], text=True))
    base = ["extension-module", "store"]
    if version >= [3, 11]:
        base.append("pyo3/abi3-py311")
    cases = []
    for name, options, user_features, cargo_flags in [
        ("default", [], [], []),
        ("unqualified", ["-F", "strict-pid-guard"], ["strict-pid-guard"], []),
        ("qualified", ["-F", "varena/strict-pid-guard"], ["varena/strict-pid-guard"], []),
        ("all_python_features", ["--all-features"], [], ["--all-features"]),
    ]:
        _, command, environment = module.build_command([
            "--manifest-path", str(manifest), "--offline", "-i", args.interpreter, *options,
        ])
        staged_features = command[command.index("--features") + 1].split(",")
        assert "--no-default-features" in command and "--all-features" not in command
        staged = tree(manifest, ["tensorfs", "tensorfs-py"], staged_features, ["--no-default-features"])
        extension = tree(manifest, ["tensorfs-py"], base + user_features, cargo_flags)
        assert staged == extension, {"case": name, "stage": staged, "extension": extension}
        native = {key: value for key, value in staged.items() if key.startswith("tensorfs v")}
        assert native and all(value == ["store"] for value in native.values()), native
        cases.append({"name": name, "package_nodes": len(staged), "features": staged_features,
                      "native_tensorfs": native, "graphs_equal": True,
                      "interpreter": environment["PYO3_PYTHON"]})
    old = tree(manifest, ["tensorfs", "tensorfs-py"], base, [])
    old_native = {key: value for key, value in old.items() if key.startswith("tensorfs v")}
    assert old_native and all("default" in value for value in old_native.values()), old_native
    # Check options against the real helper; this does not spawn cargo build.
    _, forwarded, _ = module.build_command(["--manifest-path", str(manifest), "--profile", "dev",
        "--target", "x86_64-unknown-linux-gnu", "--target-dir", "target/feature-proof",
        "--jobs", "1", "--config", "net.retry=1", "--offline", "--frozen", "-i", args.interpreter])
    for option, expected in [("--profile", "dev"), ("--target", "x86_64-unknown-linux-gnu"),
                             ("--target-dir", "target/feature-proof"), ("--jobs", "1"),
                             ("--config", "net.retry=1")]:
        assert forwarded[forwarded.index(option) + 1] == expected
    for options in [["--no-default-features"], ["--find-interpreter"], ["--release", "--profile", "dev"]]:
        try:
            module.build_command(options)
        except ValueError:
            continue
        raise AssertionError({"required_refusal": options})
    result = {"passed": True, "python": version, "cases": cases,
              "old_native_defaults_reproduced": old_native,
              "option_forwarding": True, "full_wheel_refusals": True,
              "scope": "real Cargo tree graphs only; no build/cache-speed/GPU claim"}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    result = run()
    print(json.dumps({"passed": result["passed"], "python": result["python"], "cases": len(result["cases"])}))
