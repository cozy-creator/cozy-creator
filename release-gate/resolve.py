"""resolve.py [--hub URL] [--rc X.Y.Z] <runtime wheel|version|""> <tensorfs wheel|version|""> [package dir...]: whether the
candidate set resolves on the pod, before any rental (44195951427 rented a 4090 for 10 min to learn that its Runtime refused its
TensorFS; 44238862903 ran RC Runtime 0.48.0.dev2333 on the Hub's TensorFS 0.26.0, which lacks what it calls: xs-216).

A member not given is the one the machine will run. An RC Runtime build (`0.48.0.dev2333+g…`) with no TensorFS takes the RC's
TensorFS build beside it, the RC's set together: the newest on cozy-creator's `rc-<--rc>` its requirement admits, printed as
`tensorfs=<URL>#sha256=<hex>` for the gate to take. Any other missing member is the Hub's target (`--hub`'s /v1/software), or
with no `--hub` (a machine run as it is) unknown and unchecked.
The Runtime's own tensorfs requirements (its wheel's, or its release's on PyPI) must admit that TensorFS, and each package's
cozy-runtime requirement that Runtime. A pod is Linux x86_64: a requirement for another platform doesn't count.
Prints each conflict, a line each; exit 1 on any."""
import argparse
import json
import sys
import tomllib
import urllib.error
import urllib.request
import zipfile
from email.parser import Parser
from pathlib import Path
from typing import cast

from packaging.markers import default_environment
from packaging.requirements import Requirement
from packaging.version import InvalidVersion, Version

# A pod, as a requirement's marker reads it.
POD = cast("dict[str, str]", {**default_environment(), "sys_platform": "linux", "platform_machine": "x86_64", "platform_system": "Linux",
                              "os_name": "posix", "extra": ""})


def read(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "cozy-release-gate"})
    with urllib.request.urlopen(request, timeout=60) as answer:
        return cast(bytes, answer.read())


def wheel(path: str) -> tuple[Version, list[str]] | None:
    """A wheel file's version and Requires-Dist lines."""
    if not path.endswith(".whl") or not Path(path).is_file():
        return None
    with zipfile.ZipFile(path) as file:
        name = next(n for n in file.namelist() if n.endswith(".dist-info/METADATA"))
        metadata = Parser().parsestr(file.read(name).decode())
    return Version(str(metadata["Version"])), [str(line) for line in metadata.get_all("Requires-Dist") or []]


def released(distribution: str, version: str) -> tuple[Version, list[str]] | None:
    """A release's version and Requires-Dist lines, as PyPI says them; unread (a GitHub prerelease, no answer): its version alone."""
    try:
        named = Version(version)
    except InvalidVersion:
        return None
    try:
        info = json.loads(read(f"https://pypi.org/pypi/{distribution}/{version}/json"))["info"]
    except (urllib.error.URLError, TimeoutError, ValueError, KeyError):
        return named, []
    return named, [str(line) for line in info.get("requires_dist") or []]


def needs(requires: list[str], distribution: str) -> list[Requirement]:
    """What `requires` asks of `distribution` on a pod."""
    found = [Requirement(line) for line in requires]
    return [need for need in found if need.name == distribution and (need.marker is None or need.marker.evaluate(POD))]


def rc_build(version: Version) -> bool:
    """A build of the open RC: a dev release of the minor it makes, stamped with its commit (`0.48.0.dev2333+g1a2b3c4d`)."""
    return version.is_devrelease and not version.pre and version.post is None and (version.local or "").startswith("g")


def wants(admits: list[Requirement]) -> str:
    return ", ".join(f"{need.name}{need.specifier}" for need in admits)


def rc_tensorfs(rc: str, admits: list[Requirement]) -> tuple[Version, str] | None:
    """The newest TensorFS build on cozy-creator's `rc-<rc>` that `admits` take, and its URL#sha256. Its SHA256SUMS lists the
    newest first (rc-dev.yaml; SHA256SUMS.old while it swaps): of one version's builds (`0.26.0+dev.g…`), the first listed."""
    base = f"https://github.com/cozy-creator/cozy-creator/releases/download/rc-{rc}"
    try:
        sums = read(f"{base}/SHA256SUMS")
    except urllib.error.HTTPError:
        sums = read(f"{base}/SHA256SUMS.old")
    newest: tuple[Version, str] | None = None
    for line in sums.decode().splitlines():
        digest, _, file = line.partition(" ")
        file = file.strip().lstrip("*")
        if not (file.startswith("tensorfs-") and file.endswith("-abi3-manylinux_2_28_x86_64.whl")):
            continue
        version = Version(file.split("-")[1])
        if all(need.specifier.contains(version, prereleases=True) for need in admits) and (
                newest is None or Version(version.public) > Version(newest[0].public)):
            newest = version, f"{base}/{file}#sha256={digest}"
    return newest


def conflicts(runtime: str, tensorfs: str, packages: list[str], hub: str, rc: str) -> tuple[list[str], list[str]]:
    said: list[str] = []
    takes: list[str] = []
    target: dict[str, str] = {}

    def hubs(member: str) -> str:
        if hub and not target:
            target.update({key: str(value) for key, value in json.loads(read(f"{hub.rstrip('/')}/v1/software")).items()})
        return target.get(member, "")

    rt, rt_from = wheel(runtime), "the candidate"
    if rt is None:
        rt, rt_from = released("cozy-runtime", runtime or hubs("runtime")), "the candidate" if runtime else "the Hub's target"
    tf, tf_from = wheel(tensorfs), "the candidate"
    if tf is None and tensorfs:
        tf = released("tensorfs", tensorfs)
    if tf is None and rt is not None and rc_build(rt[0]):
        if not rc:
            return [f"cozy-runtime {rt[0]} is an RC build and no TensorFS was given: give tensorfs_wheel, or dispatch from the RC's branch"], []
        found = rc_tensorfs(rc, needs(rt[1], "tensorfs"))
        if found is None:
            return [f"cozy-runtime {rt[0]} needs {wants(needs(rt[1], 'tensorfs'))}; rc-{rc} holds no TensorFS build it admits"], []
        tf, tf_from = (found[0], []), f"rc-{rc}'s"
        takes.append(f"tensorfs={found[1]}")
    if tf is None:
        tf, tf_from = released("tensorfs", hubs("tensorfs")), "the Hub's target"
    if rt is not None and tf is not None:
        for need in needs(rt[1], "tensorfs"):
            if not need.specifier.contains(tf[0], prereleases=True):
                said.append(f"cozy-runtime {rt[0]} ({rt_from}) needs {wants([need])}; the machine would run tensorfs {tf[0]} ({tf_from})")
    if rt is not None:
        for package in packages:
            with open(Path(package) / "pyproject.toml", "rb") as f:
                project = tomllib.load(f).get("project", {})
            for need in needs([str(line) for line in project.get("dependencies", [])], "cozy-runtime"):
                if not need.specifier.contains(rt[0], prereleases=True):
                    said.append(f"{project.get('name', package)} needs {wants([need])}; the machine would run cozy-runtime {rt[0]} ({rt_from})")
    return sorted(set(said)), takes


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--hub", default="")
    parser.add_argument("--rc", default="")
    parser.add_argument("runtime")
    parser.add_argument("tensorfs")
    parser.add_argument("packages", nargs="*")
    args = parser.parse_args()
    try:
        found, taken = conflicts(args.runtime, args.tensorfs, args.packages, args.hub, args.rc)
    except (urllib.error.URLError, TimeoutError) as e:
        found, taken = [f"the set the machine would run couldn't be read ({getattr(e, 'url', '') or args.hub}: {e})"], []
    print("\n".join(found or taken))
    sys.exit(1 if found else 0)
