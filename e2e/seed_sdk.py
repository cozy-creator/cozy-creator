# /// script
# requires-python = ">=3.12"
# dependencies = ["packaging>=24", "tomli-w>=1.2"]
# ///
"""The selected SDK, served by the existing isolated-Hub ingress for copied packages."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import platform
import subprocess
import sys
from dataclasses import asdict, dataclass
from email.parser import BytesParser
from pathlib import Path
from urllib.parse import parse_qs, quote, unquote, urlsplit
from urllib.request import urlopen
from zipfile import ZipFile

import tomli_w
import tomllib
from packaging.requirements import Requirement
from packaging.tags import sys_tags
from packaging.utils import canonicalize_name, parse_wheel_filename
from packaging.version import Version


@dataclass(frozen=True)
class SeedWheel:
    name: str
    version: str
    filename: str
    sha256: str
    selected: str
    commit: str = ""

    def url(self, origin: str) -> str:
        return f"{origin.rstrip('/')}/__cozy_sdk/{self.sha256}/{quote(self.filename)}"


@dataclass(frozen=True)
class SeedSdk:
    runtime: SeedWheel
    tensorfs: SeedWheel | None
    origin: str = ""


def _download(url: str) -> bytes:
    with urlopen(url, timeout=60) as response:
        data: bytes = response.read()
        return data


def _release(value: str, distribution: str) -> tuple[str, str]:
    version = value.removeprefix("v")
    Version(version)
    repo, tag = (
        ("cozy-runtime", f"v{version}")
        if distribution == "cozy-runtime"
        else ("cozy-creator", f"tensorfs-v{version}")
    )
    base = f"https://github.com/cozy-creator/{repo}/releases/download/{tag}"
    tags = list(sys_tags())
    choices: list[tuple[int, str, str]] = []
    for line in _download(f"{base}/SHA256SUMS").decode().splitlines():
        fields = line.split()
        if len(fields) != 2 or not fields[1].lstrip("*").endswith(".whl"):
            continue
        digest, filename = fields[0], fields[1].lstrip("*")
        name, found_version, _, supported = parse_wheel_filename(filename)
        if name != distribution or found_version != Version(version):
            continue
        ranks = [i for i, tag in enumerate(tags) if tag in supported]
        if ranks:
            # A pure Runtime is the same author reader on every platform.
            rank = min(ranks)
            if filename.endswith("-py3-none-any.whl"):
                rank = -2
            elif "-abi3-" in filename:
                rank = -1
            choices.append((rank, filename, digest))
    if not choices:
        raise ValueError(f"{distribution} {version} has no wheel for this reader")
    _, filename, digest = min(choices)
    return f"{base}/{quote(filename)}", digest


def _stage(value: str, distribution: str, directory: Path) -> SeedWheel:
    source = urlsplit(value)
    expected = ""
    if source.scheme in ("http", "https"):
        expected = parse_qs(source.fragment).get("sha256", [""])[0]
        if len(expected) != 64:
            raise ValueError("an explicit SDK URL needs #sha256=<digest>")
        data = _download(value)
    elif Path(value).is_file():
        data = Path(value).read_bytes()
    else:
        url, expected = _release(value, distribution)
        data = _download(url)
    digest = hashlib.sha256(data).hexdigest()
    if expected and digest != expected:
        raise ValueError(f"{distribution}: selected wheel checksum differs")
    incoming = directory / f"{distribution}-{digest}.zip"
    incoming.write_bytes(data)
    try:
        with ZipFile(incoming) as wheel:
            metadata = BytesParser().parsebytes(
                wheel.read(
                    next(
                        n for n in wheel.namelist() if n.endswith(".dist-info/METADATA")
                    )
                )
            )
            name, version = str(metadata["Name"]), str(metadata["Version"])
            if canonicalize_name(name) != distribution:
                raise ValueError(f"selected {distribution} file contains {name}")
            tags = (
                BytesParser()
                .parsebytes(
                    wheel.read(
                        next(
                            n
                            for n in wheel.namelist()
                            if n.endswith(".dist-info/WHEEL")
                        )
                    )
                )
                .get_all("Tag", [])
            )
            # A caller may have renamed its file; preserve the actual wheel's name/version/tag.
            filename = f"{distribution.replace('-', '_')}-{version}-{tags[0]}.whl"
            commit = ""
            provenance = "cozy_runtime/_build_provenance.py"
            if provenance in wheel.namelist():
                for node in ast.parse(wheel.read(provenance)).body:
                    if isinstance(node, ast.Assign) and any(
                        isinstance(t, ast.Name) and t.id == "COMMIT"
                        for t in node.targets
                    ):
                        commit = str(ast.literal_eval(node.value))
    finally:
        incoming.unlink()
    destination = directory / "sdk" / digest / filename
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(data)
    return SeedWheel(distribution, version, filename, digest, value, commit)


def prepare_seed_sdk(runtime: str, tensorfs: str, directory: Path) -> SeedSdk:
    """Materialize exactly the selected inputs; never select a rolling RC here."""
    if not runtime:
        raise ValueError("isolated package seeding needs the selected Runtime")
    directory.mkdir(parents=True, exist_ok=True)
    return SeedSdk(
        _stage(runtime, "cozy-runtime", directory),
        _stage(tensorfs, "tensorfs", directory) if tensorfs else None,
    )


def pin_seed_sdk(project: Path, sdk: SeedSdk) -> None:
    """Pin a fixture copy without changing its authored version ranges or source code."""
    if not sdk.origin:
        raise ValueError("the SDK fixture ingress has no origin")
    path = project / "pyproject.toml"
    document = tomllib.loads(path.read_text())
    project_table = document["project"]
    dependencies = project_table.get("dependencies", [])
    tool = document.setdefault("tool", {})
    uv = tool.setdefault("uv", {})
    sources = uv.setdefault("sources", {})
    if sdk.tensorfs is None or any(
        wheel is not None and not wheel.filename.endswith("-none-any.whl")
        for wheel in (sdk.runtime, sdk.tensorfs)
    ):
        # This fixture locks the native candidate for this runner, not unavailable wheels
        # for the other OS jobs. Each OS seeds its own isolated Hub and SDK cohort.
        # A runner with no TensorFS in its cohort (it runs no models: macOS, Windows) locks
        # for itself too: the RC Runtime's floor on a rental's platform is a TensorFS build
        # only a Linux cohort holds (xs-216).
        runner = f"sys_platform == '{sys.platform}' and platform_machine == '{platform.machine()}'"
        authored = " or ".join(f"({marker})" for marker in uv.get("environments", []))
        # uv rejects an impossible alternative even when another matches. Intersect the
        # authored union once; a wholly disjoint union still fails uv's requirement check.
        uv["environments"] = [f"({authored}) and ({runner})" if authored else runner]
    for wheel in (sdk.runtime, sdk.tensorfs):
        if wheel is None:
            continue
        declared = [
            Requirement(str(r))
            for r in dependencies
            if canonicalize_name(Requirement(str(r)).name) == wheel.name
        ]
        for requirement in declared:
            if requirement.url or not requirement.specifier.contains(
                wheel.version, prereleases=True
            ):
                raise ValueError(
                    f"{project.name}: selected {wheel.name} {wheel.version} does not meet {requirement}"
                )
        if not declared:
            # Make the selected transitive TensorFS source participate in uv resolution.
            dependencies.append(wheel.name)
        sources[wheel.name] = {"url": wheel.url(sdk.origin)}
    project_table["dependencies"] = dependencies
    path.write_text(tomli_w.dumps(document))


def read_sdk(path: Path, origin: str) -> SeedSdk:
    document = json.loads(path.read_text())
    return SeedSdk(
        SeedWheel(**document["runtime"]),
        SeedWheel(**document["tensorfs"]) if document["tensorfs"] else None,
        origin,
    )


def verify_seed_lock(project: Path, sdk: SeedSdk, receipt: Path) -> None:
    """A publication's actual lock must still name the selected bytes."""
    lock = project / "uv.lock"
    packages = tomllib.loads(lock.read_text())["package"]
    for selected in (sdk.runtime, sdk.tensorfs):
        if selected is None:
            continue
        row = next(p for p in packages if canonicalize_name(p["name"]) == selected.name)
        if row["version"] != selected.version or unquote(
            row["source"].get("url", "")
        ) != unquote(selected.url(sdk.origin)):
            raise ValueError(
                f"{project.name}: publication changed selected {selected.name}"
            )
        if not any(
            w.get("hash") == "sha256:" + selected.sha256 for w in row.get("wheels", [])
        ):
            raise ValueError(
                f"{project.name}: publication changed {selected.name}'s bytes"
            )
    document = json.loads(receipt.read_text())
    document["verified_locks"].append(
        {
            "package": project.name,
            "lock_sha256": hashlib.sha256(lock.read_bytes()).hexdigest(),
        }
    )
    receipt.write_text(json.dumps(document, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--runtime", required=True)
    prepare.add_argument("--tensorfs", default="")
    prepare.add_argument("--directory", type=Path, required=True)
    pin = commands.add_parser("pin")
    pin.add_argument("--project", type=Path, required=True)
    pin.add_argument("--cohort", type=Path, required=True)
    pin.add_argument("--origin", required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("--project", type=Path, required=True)
    verify.add_argument("--cohort", type=Path, required=True)
    verify.add_argument("--origin", required=True)
    verify.add_argument("--receipt", type=Path, required=True)
    record = commands.add_parser("record")
    record.add_argument("--cohort", type=Path, required=True)
    record.add_argument("--origin", required=True)
    record.add_argument("--packages", type=Path)
    record.add_argument("--runtime-ref", default="")
    record.add_argument("--packages-ref", default="")
    record.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        sdk = prepare_seed_sdk(args.runtime, args.tensorfs, args.directory)
        (args.directory / "seed-sdk.json").write_text(
            json.dumps(asdict(sdk), indent=2) + "\n"
        )
    elif args.command == "pin":
        pin_seed_sdk(args.project, read_sdk(args.cohort, args.origin))
    elif args.command == "verify":
        verify_seed_lock(args.project, read_sdk(args.cohort, args.origin), args.receipt)
    else:
        sdk = read_sdk(args.cohort, args.origin)
        packages_commit = ""
        if args.packages is not None:
            packages_commit = subprocess.check_output(
                ["git", "-C", str(args.packages), "rev-parse", "HEAD"], text=True
            ).strip()
        document: dict[str, object] = {
            "runtime_requested_ref": args.runtime_ref,
            "packages_requested_ref": args.packages_ref,
            "packages_commit": packages_commit,
            "sdk": asdict(sdk),
            "verified_locks": [],
        }
        if (
            args.packages_ref
            and len(args.packages_ref) == 40
            and packages_commit != args.packages_ref
        ):
            raise ValueError("seeding received a different packages checkout")
        args.receipt.write_text(json.dumps(document, indent=2) + "\n")
        print(
            f"Seed SDK: Runtime {sdk.runtime.version}, sha256 {sdk.runtime.sha256}, source {sdk.runtime.commit or '(release)'}"
        )
        print(f"Seed packages: {packages_commit or '(none)'}")


if __name__ == "__main__":
    main()
