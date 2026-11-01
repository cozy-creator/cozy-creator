#!/usr/bin/env python3
"""Bundle private Rust dependencies for native cozy/TensorD artifacts before attestation.

Run with uv --with patchelf (Linux) or --with lief (Windows). No installs/releases.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def kind(path):
    with path.open("rb") as file:
        magic = file.read(4)
    if magic == b"\x7fELF":
        return "elf"
    if magic[:2] == b"MZ":
        return "pe"
    return "macho"


def dependencies(path):
    """Actual loader dependencies, not filenames found in a build cache."""
    image = kind(path)
    if image == "elf":
        return run("patchelf", "--print-needed", str(path)).splitlines()
    if image == "pe":
        import lief

        binary = lief.PE.parse(str(path))
        if binary is None:
            raise RuntimeError(f"cannot read PE dependencies: {path}")
        return sorted(
            {library.name for library in [*binary.imports, *binary.delay_imports]}
        )
    names = [
        line.strip().split()[0]
        for line in run("otool", "-L", str(path)).splitlines()[1:]
    ]
    # otool -L includes a dylib's own install name. It is not a dependency.
    identifiers = [
        line.strip() for line in run("otool", "-D", str(path)).splitlines()[1:]
    ]
    return [name for name in names if name not in identifiers]


def runtime_for(core, directory):
    names = {
        Path(name).name
        for name in dependencies(core)
        if Path(name).name.startswith(("libstd-", "std-"))
    }
    if len(names) != 1:
        raise RuntimeError(
            f"expected one shared Rust runtime dependency in {core}, found {sorted(names)}"
        )
    name = names.pop()
    runtime = directory / name
    if not runtime.is_file():
        raise FileNotFoundError(f"the core requires {name}, absent from {directory}")
    return runtime


def linkage(directory, manifest, triple, max_glibc=None):
    """Final-image facts; runtime resolution is checked only on a matching Linux host."""
    private = {item["name"] for item in manifest["libraries"]}
    images = []
    requirements = set()
    for name in [manifest["binary"], *sorted(private)]:
        path = directory / name
        needed = dependencies(path)
        for dependency in needed:
            basename = Path(dependency).name
            if (
                basename.startswith(("libvarena", "varena", "libstd-", "std-"))
                and basename not in private
            ):
                raise RuntimeError(
                    f"{name} still depends on an unbundled private library: {dependency}"
                )
        row = {
            "name": name,
            "sha256": digest(path),
            "format": kind(path),
            "needed": needed,
        }
        if row["format"] == "elf":
            row["rpath"] = run("patchelf", "--print-rpath", str(path))
            versions = run("readelf", "--version-info", str(path))
            glibc = sorted(
                set(re.findall(r"Name: GLIBC_([0-9]+(?:\.[0-9]+)+)", versions)),
                key=lambda v: tuple(map(int, v.split("."))),
            )
            row["required_glibc_versions"] = glibc
            requirements.update(glibc)
        elif row["format"] == "macho":
            commands = run("otool", "-l", str(path))
            minimums = []
            for block in re.split(r"\nLoad command [0-9]+\n", commands):
                if re.search(r"^\s*cmd LC_BUILD_VERSION\s*$", block, re.MULTILINE):
                    match = re.search(r"^\s*minos (\S+)", block, re.MULTILINE)
                    if match:
                        minimums.append(
                            {"command": "LC_BUILD_VERSION", "minimum": match[1]}
                        )
                elif re.search(
                    r"^\s*cmd LC_VERSION_MIN_MACOSX\s*$", block, re.MULTILINE
                ):
                    match = re.search(r"^\s*version (\S+)", block, re.MULTILINE)
                    if match:
                        minimums.append(
                            {"command": "LC_VERSION_MIN_MACOSX", "minimum": match[1]}
                        )
            row["minimum_os_load_commands"] = minimums
        images.append(row)
    receipt = {
        "schema": 1,
        "target": triple,
        "scope": "final packaged image dependencies; not application, GPU or platform qualification",
        "images": images,
        "application_execution": "not run",
    }
    if requirements:
        receipt["required_glibc"] = max(
            requirements, key=lambda v: tuple(map(int, v.split(".")))
        )
    if max_glibc is not None:
        if not re.fullmatch(r"[0-9]+(?:\.[0-9]+)+", max_glibc):
            raise ValueError("--max-glibc must be a numeric version")
        if requirements and tuple(
            map(int, receipt["required_glibc"].split("."))
        ) > tuple(map(int, max_glibc.split("."))):
            raise RuntimeError(
                f"packaged images require glibc {receipt['required_glibc']}, above the declared {max_glibc} release limit"
            )
        if any(row["format"] == "elf" for row in images):
            receipt["maximum_glibc"] = max_glibc
    binary = directory / manifest["binary"]
    if kind(binary) == "elf" and sys.platform == "linux":
        with binary.open("rb") as file:
            header = file.read(20)
        machine = struct.unpack_from("<H", header, 18)[0]
        native = {"x86_64": 62, "aarch64": 183}.get(platform.machine()) == machine
        if native:
            environment = {
                key: value
                for key, value in os.environ.items()
                if key not in ("LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT")
            }
            result = subprocess.run(
                ["ldd", str(binary)],
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )
            if "not found" in result.stdout or result.returncode:
                raise RuntimeError(
                    f"packaged loader resolution failed for {binary}: {result.stdout}{result.stderr}"
                )
            resolved = dict(
                re.findall(r"^\s*(\S+)\s+=>\s+(\S+)", result.stdout, re.MULTILINE)
            )
            for name in private:
                if (
                    name not in resolved
                    or Path(resolved[name]).resolve() != (directory / name).resolve()
                ):
                    raise RuntimeError(
                        f"{name} did not resolve to its packaged file: {resolved.get(name)}"
                    )
            receipt["linux_loader"] = {
                "build_environment_removed": True,
                "ignored_environment": ["LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"],
                "resolved": resolved,
                "output": result.stdout,
            }
        else:
            receipt["linux_loader"] = {
                "checked": False,
                "reason": "host and artifact architectures differ",
            }
    return receipt


def patch(path, replacements):
    if kind(path) == "elf":
        for needed in run("patchelf", "--print-needed", str(path)).splitlines():
            if needed in replacements:
                run(
                    "patchelf",
                    "--replace-needed",
                    needed,
                    replacements[needed],
                    str(path),
                )
        run("patchelf", "--set-rpath", "$ORIGIN", str(path))
    elif kind(path) == "pe":
        import lief

        binary = lief.PE.parse(str(path))
        for library in binary.imports:
            if library.name in replacements:
                library.name = replacements[library.name]
        config = lief.PE.Builder.config_t()
        config.imports = True
        binary.write(str(path), config)
    else:
        dependencies = run("otool", "-L", str(path)).splitlines()[1:]
        for line in dependencies:
            name = line.strip().split()[0]
            if Path(name).name in replacements:
                run(
                    "install_name_tool",
                    "-change",
                    name,
                    "@loader_path/" + replacements[Path(name).name],
                    str(path),
                )
        # install_name_tool invalidates the ad-hoc signature on Apple Silicon.
        run("codesign", "--force", "--sign", "-", str(path))


def bundle(
    binary: Path,
    target: Path,
    output: Path,
    triple: str | None,
    runtime_dir: Path | None = None,
    max_glibc: str | None = None,
):
    output.mkdir(parents=True, exist_ok=True)
    candidates = [
        target / "deps" / name
        for name in ("libvarena.so", "libvarena.dylib", "varena.dll")
    ]
    (core,) = [path for path in candidates if path.is_file()]
    command = ["rustc", "--print", "target-libdir"]
    if triple:
        command.extend(["--target", triple])
    runtime_dir = runtime_dir or Path(run(*command))
    runtime = runtime_for(core, runtime_dir)
    runtime_name = (
        ("std-private-" if core.suffix == ".dll" else "libstd-private-")
        + digest(runtime)[:32]
        + runtime.suffix
    )
    shutil.copy2(runtime, output / runtime_name)
    # Keep the core's canonical SONAME/install-name. Matching external bindings may
    # reuse it; only the internal executable's dependency filename is private.
    with tempfile.TemporaryDirectory(
        prefix="native-link-", dir=output.parent
    ) as temporary:
        prepared = Path(temporary) / core.name
        shutil.copy2(core, prepared)
        patch(prepared, {runtime.name: runtime_name})
        core_name = (
            ("varena-private-" if core.suffix == ".dll" else "libvarena-private-")
            + digest(prepared)[:32]
            + core.suffix
        )
        shutil.copy2(prepared, output / core_name)
    destination = output / binary.name
    if binary.resolve() != destination.resolve():
        shutil.copy2(binary, destination)
    patch(destination, {core.name: core_name, runtime.name: runtime_name})
    manifest = {
        "schema": 1,
        "binary": binary.name,
        "libraries": [
            {"name": name, "sha256": digest(output / name)}
            for name in (core_name, runtime_name)
        ],
    }
    (output / "varena-native.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "native-linkage.json").write_text(
        json.dumps(linkage(output, manifest, triple, max_glibc), indent=2) + "\n"
    )
    return manifest


def archive(directory: Path, destination: Path, prefix: str = ""):
    if destination.suffix == ".zip":
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as writer:
            for file in sorted(directory.rglob("*")):
                if file.is_file():
                    writer.write(file, prefix + file.relative_to(directory).as_posix())
    else:
        with tarfile.open(destination, "w:gz") as writer:
            for file in sorted(directory.iterdir()):
                writer.add(file, arcname=prefix + file.name)


def owned_archives(manifest, target_root, targets):
    if not targets or any(
        not isinstance(target, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", target)
        for target in targets
    ):
        raise ValueError("dist requires explicit valid matrix target ownership")
    selected = []
    for name, artifact in manifest["artifacts"].items():
        if artifact.get("kind") != "executable-zip":
            continue
        triples = set(artifact.get("target_triples", []))
        if not triples.intersection(targets):
            continue  # Never inspect or mutate another job's downloaded archive.
        if len(triples) != 1 or not triples.issubset(targets):
            raise ValueError(
                f"unsupported or partly owned multi-target archive: {name}"
            )
        if Path(name).name != name:
            raise ValueError(f"invalid owned archive name: {name}")
        path = target_root / "distrib" / name
        if not path.is_file():
            raise FileNotFoundError(f"owned archive is missing: {path}")
        selected.append((name, artifact, path, next(iter(triples))))
    if not selected:
        raise ValueError(
            f"no executable archive belongs to matrix targets {sorted(targets)}"
        )
    return selected


def dist(manifest_path: Path, target_root: Path, targets, max_glibc=None):
    manifest = json.loads(manifest_path.read_text())
    for name, artifact, path, triple in owned_archives(manifest, target_root, targets):
        profile = target_root / triple / "dist"
        if not profile.is_dir():
            profile = target_root / triple / "release"
        with tempfile.TemporaryDirectory(
            prefix="native-archive-", dir=path.parent
        ) as temporary:
            unpacked = Path(temporary)
            if name.endswith(".zip"):
                with zipfile.ZipFile(path) as source:
                    source.extractall(unpacked)
                directory, prefix, binary_name = unpacked, "", "cozy.exe"
            else:
                with tarfile.open(path) as source:
                    source.extractall(unpacked, filter="data")
                prefix = name.removesuffix(".tar.gz") + "/"
                directory, binary_name = unpacked / prefix, "cozy"
            bundle(
                directory / binary_name, profile, directory, triple, max_glibc=max_glibc
            )
            archive(directory, path, prefix)
            # cargo-dist collected linkage before the final dependency rewrite. Do
            # not publish that stale field as evidence about these new archive bytes.
            # The attested archive carries native-linkage.json from the final images.
            for asset in artifact.get("assets", []):
                if asset.get("kind") == "executable" and asset.get(
                    "id"
                ) in manifest.get("assets", {}):
                    manifest["assets"][asset["id"]].pop("linkage", None)
        sha = digest(path)
        artifact.setdefault("checksums", {})["sha256"] = sha
        (path.parent / (name + ".sha256")).write_text(f"{sha}  {name}\n")
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--target", type=Path)
    parser.add_argument("--triple")
    parser.add_argument("--runtime-dir", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--dist-manifest", type=Path)
    parser.add_argument(
        "--targets-json", help="exact JSON matrix.targets owned by this build job"
    )
    parser.add_argument(
        "--max-glibc",
        help="enforce this ELF version-requirement ceiling (release archives: 2.28)",
    )
    parser.add_argument("--target-root", type=Path, default=Path("target"))
    args = parser.parse_args()
    if args.dist_manifest:
        if args.targets_json is None:
            parser.error("--dist-manifest requires --targets-json")
        targets = json.loads(args.targets_json)
        if not isinstance(targets, list) or any(
            not isinstance(target, str) for target in targets
        ):
            parser.error("--targets-json must be an array of target strings")
        dist(args.dist_manifest, args.target_root, set(targets), args.max_glibc)
    else:
        if not all((args.binary, args.target, args.out)):
            parser.error("--binary, --target and --out are required")
        value = bundle(
            args.binary.resolve(),
            args.target.resolve(),
            args.out.resolve(),
            args.triple,
            args.runtime_dir,
            args.max_glibc,
        )
        if args.archive:
            archive(args.out.resolve(), args.archive.resolve())
        print(json.dumps(value))


if __name__ == "__main__":
    main()
