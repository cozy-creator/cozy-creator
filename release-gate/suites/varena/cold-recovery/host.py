"""Run actual native cache-worker host controls from an exact output source copy."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import tomllib


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    fixture = args.out / "native"
    shutil.copytree(args.source / "crates/varena/src", fixture / "src")
    shutil.copytree(args.source / "crates/platform-signal", args.out / "platform-signal")
    workspace = tomllib.loads((args.source / "Cargo.toml").read_text())["workspace"]
    crate = tomllib.loads((args.source / "crates/varena/Cargo.toml").read_text())

    def toml(value):
        if isinstance(value, dict):
            return "{" + ", ".join(f"{key} = {toml(item)}" for key, item in value.items()) + "}"
        return json.dumps(value)

    lines = ["[package]", 'name = "varena"', f'version = "{crate["package"]["version"]}"',
             f'edition = "{workspace["package"]["edition"]}"', "[workspace]", "[lib]",
             'crate-type = ["dylib"]', "[dependencies]", 'platform-signal = { path = "../platform-signal" }']
    for name in ("libloading", "thiserror", "libc", "nvml-wrapper", "nvml-wrapper-sys", "serde_json"):
        lines.append(f"{name} = {toml(workspace['dependencies'][name])}")
    lines.append("[features]")
    lines.extend(f"{name} = {toml(value)}" for name, value in crate["features"].items())
    (fixture / "Cargo.toml").write_text("\n".join(lines) + "\n")
    signal = args.out / "platform-signal/Cargo.toml"
    signal_manifest = tomllib.loads(signal.read_text())
    lines = ["[package]"]
    for name, value in signal_manifest["package"].items():
        if isinstance(value, dict) and value.get("workspace"):
            value = workspace["package"][name]
        lines.append(f"{name} = {toml(value)}")
    for target, tables in signal_manifest["target"].items():
        lines.append(f"[target.{json.dumps(target)}.dependencies]")
        for name, value in tables["dependencies"].items():
            inherited = workspace["dependencies"][name]
            inherited = {"version": inherited} if isinstance(inherited, str) else dict(inherited)
            inherited["features"] = list(dict.fromkeys(inherited.get("features", []) + value.get("features", [])))
            lines.append(f"{name} = {toml(inherited)}")
    signal.write_text("\n".join(lines) + "\n")
    shutil.copy2(args.source / "Cargo.lock", fixture / "Cargo.lock")
    harness = Path(__file__).with_name("host.rs")
    with (fixture / "src/cache_owner.rs").open("a") as output:
        output.write("\n" + harness.read_text())
    (fixture / "examples").mkdir()
    (fixture / "examples/cold_host.rs").write_text(
        'fn main() { println!("{}", varena::cache_owner::cold_recovery_host_proof().unwrap()); }\n')
    command = ["cargo", "run", "--offline", "--manifest-path", str(fixture / "Cargo.toml"),
               "--target-dir", str(args.target), "--example", "cold_host", "-j", "1"]
    start = time.monotonic()
    environment = {**os.environ, "CARGO_BUILD_JOBS": "1", "RUSTFLAGS": "-C prefer-dynamic"}
    with (args.out / "stdout").open("w") as stdout, (args.out / "stderr").open("w") as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr, check=False,
                                env=environment, timeout=600)
    report = {"command": command, "exit_code": result.returncode, "seconds": time.monotonic() - start,
              "rustflags": environment["RUSTFLAGS"],
              "source_hashes": {str(p.relative_to(args.source)): hashlib.sha256(p.read_bytes()).hexdigest()
                                for p in (args.source / "crates/varena/src").rglob("*.rs")},
              "harness_sha256": hashlib.sha256(harness.read_bytes()).hexdigest(),
              "scope": "actual native host control; no simulated physical release and no GPU qualification"}
    if result.returncode == 0:
        report["result"] = json.loads((args.out / "stdout").read_text())
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
