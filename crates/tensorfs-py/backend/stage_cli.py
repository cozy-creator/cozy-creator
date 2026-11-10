"""Build/stage the CLI with the same Cargo options as the extension build."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import TypedDict


class Options(argparse.Namespace):
    manifest_path: str | None
    profile: str | None
    release: bool
    target: str | None
    target_dir: str | None
    features: list[str]
    all_features: bool
    no_default_features: bool
    config: list[str]
    offline: bool
    frozen: bool
    jobs: str | None
    interpreter: list[str] | None
    find_interpreter: bool


class CargoPackage(TypedDict):
    name: str
    manifest_path: str
    features: dict[str, list[str]]


class CargoMetadata(TypedDict):
    packages: list[CargoPackage]


class CargoTarget(TypedDict):
    name: str


class CargoMessage(TypedDict, total=False):
    """A `cargo build --message-format=json` line's fields read here (total=False: Python 3.10 has no NotRequired)."""

    reason: str
    target: CargoTarget
    executable: str | None


def build_command(
    args: list[str], default_profile: str = "release"
) -> tuple[Path, list[str], dict[str, str]]:
    parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
    parser.add_argument("-m", "--manifest-path")
    parser.add_argument("--profile")
    parser.add_argument("-r", "--release", action="store_true")
    parser.add_argument("--target")
    parser.add_argument("--target-dir")
    parser.add_argument("-F", "--features", action="append", default=[])
    parser.add_argument("--all-features", action="store_true")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--config", action="append", default=[])
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--frozen", action="store_true")
    parser.add_argument("-j", "--jobs")
    parser.add_argument("-i", "--interpreter", nargs="+")
    parser.add_argument("--find-interpreter", action="store_true")
    options, _ = parser.parse_known_args(args, namespace=Options())
    if options.no_default_features or options.find_interpreter:
        raise ValueError(
            "the full TensorFS wheel needs explicit interpreters and store support"
        )
    if options.release and options.profile:
        raise ValueError("choose --release or --profile, not both")
    interpreter = (options.interpreter or [sys.executable])[0]
    version: list[int] = json.loads(
        subprocess.check_output(
            [
                interpreter,
                "-c",
                "import json,sys; print(json.dumps(list(sys.version_info[:2])))",
            ],
            text=True,
        )
    )
    root = (
        Path(options.manifest_path).resolve().parent
        if options.manifest_path
        else Path(__file__).resolve().parents[1]
    )
    # Cargo applies defaults to every explicitly selected package. Maturin selects
    # only tensorfs-py, whose tensorfs dependency disables native package defaults.
    # Re-enable Python's defaults explicitly, never tensorfs/default.
    features = [
        "tensorfs-py/default",
        "tensorfs-py/extension-module",
        "tensorfs-py/store",
    ]
    if version >= [3, 11]:
        features.append("pyo3/abi3-py311")
    for value in options.features:
        features.extend(
            feature if "/" in feature else "tensorfs-py/" + feature
            for feature in value.replace(",", " ").split()
        )
    if options.all_features:
        # Discover declared Python-package features through Cargo JSON so this works
        # on Python 3.10 and follows future manifest additions without TOML parsing.
        metadata = [
            "cargo", "metadata", "--format-version=1", "--no-deps", "--locked",
            "--manifest-path", str(root / "Cargo.toml"),
        ]
        for value in options.config:
            metadata.extend(["--config", value])
        for flag in ("offline", "frozen"):
            if getattr(options, flag):
                metadata.append("--" + flag)
        cargo: CargoMetadata = json.loads(subprocess.check_output(metadata, text=True))
        packages = cargo["packages"]
        package = next(
            (package for package in packages
             if package["name"] == "tensorfs-py"
             and Path(package["manifest_path"]).resolve() == root / "Cargo.toml"),
            None,
        )
        if package is None:
            raise ValueError("CLI staging requires the tensorfs-py manifest")
        features.extend("tensorfs-py/" + name for name in sorted(package["features"]))
    features = list(dict.fromkeys(features))
    command = [
        "cargo",
        "build",
        "--locked",
        "--manifest-path",
        str(root / "Cargo.toml"),
        "-p",
        "tensorfs",
        "-p",
        "tensorfs-py",
        "--lib",
        "--bin",
        "tfs",
        "--no-default-features",
        "--features",
        ",".join(features),
        "--message-format=json-render-diagnostics",
    ]
    profile = "release" if options.release else options.profile or default_profile
    command.extend(["--profile", profile])
    for flag, given in (
        ("--target", options.target),
        ("--target-dir", options.target_dir),
        ("--jobs", options.jobs),
    ):
        if given is not None:
            command.extend([flag, given])
    for value in options.config:
        command.extend(["--config", value])
    for flag in ("offline", "frozen"):
        if getattr(options, flag):
            command.append("--" + flag.replace("_", "-"))
    environment = os.environ.copy()
    environment["PYO3_PYTHON"] = shutil.which(interpreter) or interpreter
    return root, command, environment


def stage(args: list[str], default_profile: str = "release") -> Path:
    root, command, environment = build_command(args, default_profile)
    from stage_shim import stage as stage_shim

    stage_shim(root, command, environment)
    process = subprocess.Popen(
        command, stdout=subprocess.PIPE, text=True, env=environment
    )
    executable: str | None = None
    assert process.stdout is not None
    for line in process.stdout:
        message: CargoMessage = json.loads(line)
        if (
            message.get("reason") == "compiler-artifact"
            and message["target"]["name"] == "tfs"
        ):
            executable = message.get("executable") or executable
    if process.wait() or executable is None:
        raise RuntimeError("native CLI cohort build failed")
    built = Path(executable)
    staged = root / "wheel-data/scripts" / built.name
    staged.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(built, staged)
    return staged


if __name__ == "__main__":
    print(stage(sys.argv[1:]))
