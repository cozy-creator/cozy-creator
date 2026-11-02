"""Build/stage the CLI with the same Cargo options as the extension build."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


def stage(args: list[str], default_profile: str = "release") -> Path:
    parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
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
    options, _ = parser.parse_known_args(args)
    if options.no_default_features or options.find_interpreter:
        raise ValueError(
            "the full TensorFS wheel needs explicit interpreters and store support"
        )
    if options.release and options.profile:
        raise ValueError("choose --release or --profile, not both")
    interpreter = (options.interpreter or [sys.executable])[0]
    version = json.loads(
        subprocess.check_output(
            [
                interpreter,
                "-c",
                "import json,sys; print(json.dumps(list(sys.version_info[:2])))",
            ],
            text=True,
        )
    )
    features = ["extension-module", "store"]
    if version >= [3, 11]:
        features.append("pyo3/abi3-py311")
    for value in options.features:
        features.extend(value.replace(" ", ",").split(","))
    root = Path(__file__).resolve().parents[1]
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
        "--features",
        ",".join(features),
        "--message-format=json-render-diagnostics",
    ]
    profile = "release" if options.release else options.profile or default_profile
    command.extend(["--profile", profile])
    for flag, value in (
        ("--target", options.target),
        ("--target-dir", options.target_dir),
        ("--jobs", options.jobs),
    ):
        if value is not None:
            command.extend([flag, value])
    for value in options.config:
        command.extend(["--config", value])
    for flag in ("all_features", "offline", "frozen"):
        if getattr(options, flag):
            command.append("--" + flag.replace("_", "-"))
    environment = os.environ.copy()
    environment["PYO3_PYTHON"] = shutil.which(interpreter) or interpreter
    process = subprocess.Popen(
        command, stdout=subprocess.PIPE, text=True, env=environment
    )
    executable = None
    for line in process.stdout:
        message = json.loads(line)
        if (
            message.get("reason") == "compiler-artifact"
            and message["target"]["name"] == "tfs"
        ):
            executable = message.get("executable") or executable
    if process.wait() or executable is None:
        raise RuntimeError("native CLI cohort build failed")
    executable = Path(executable)
    staged = root / "wheel-data/scripts" / executable.name
    staged.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(executable, staged)
    return staged


if __name__ == "__main__":
    print(stage(sys.argv[1:]))
