"""resolve.py <runtime wheel|version|""> <tensorfs wheel|version|""> [package dir...]: whether the candidate set resolves,
before any rental (44195951427 rented a 4090 for 10 min to learn that its Runtime refused its TensorFS). The Runtime
wheel's own tensorfs requirements must admit the TensorFS wheel, and each package's cozy-runtime requirement the Runtime
wheel. A candidate named by version, not by file, is the release's, which resolved when it was published: not read here.
Prints each conflict; exit 1 on any."""
import sys
import tomllib
import zipfile
from email.message import Message
from email.parser import Parser
from pathlib import Path

from packaging.requirements import Requirement
from packaging.version import Version


def metadata(path: str) -> Message | None:
    if not path.endswith(".whl") or not Path(path).is_file():
        return None
    with zipfile.ZipFile(path) as wheel:
        name = next(n for n in wheel.namelist() if n.endswith(".dist-info/METADATA"))
        return Parser().parsestr(wheel.read(name).decode())


def conflicts(runtime: str, tensorfs: str, packages: list[str]) -> list[str]:
    said: list[str] = []
    rt, tf = metadata(runtime), metadata(tensorfs)
    if rt is not None and tf is not None:
        for line in rt.get_all("Requires-Dist") or []:
            need = Requirement(line)
            if need.name == "tensorfs" and not need.specifier.contains(Version(tf["Version"]), prereleases=True):
                said.append(f"cozy-runtime {rt['Version']} needs {need}; the candidate is tensorfs {tf['Version']}")
    if rt is not None:
        for package in packages:
            with open(Path(package) / "pyproject.toml", "rb") as f:
                project = tomllib.load(f).get("project", {})
            for line in project.get("dependencies", []):
                need = Requirement(line)
                if need.name == "cozy-runtime" and not need.specifier.contains(Version(rt["Version"]), prereleases=True):
                    said.append(f"{project.get('name', package)} needs {need}; the candidate is cozy-runtime {rt['Version']}")
    return sorted(set(said))


if __name__ == "__main__":
    found = conflicts(sys.argv[1], sys.argv[2], sys.argv[3:])
    print("\n".join(found))
    sys.exit(1 if found else 0)
