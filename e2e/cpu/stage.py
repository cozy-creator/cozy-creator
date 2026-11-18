"""stage.py <packages> <package> <dest> <runtime> [<tensorfs>]: the package as an RC machine reads it from a packages checkout (xs-195,
Calcifer's `checkout::stage`). Each `{ index = "tensorhub" }` source naming a package the checkout holds becomes a path to
that folder, copied beside it, and so on through what those take. The Runtime it is read with is `<runtime>` when that is
a wheel or a source folder, and its TensorFS `<tensorfs>` when that is a wheel (the RC's Runtime floors on a TensorFS build only
the candidate set holds, xs-216), and it is locked for this computer alone, as a machine's lock is. Prints the staged package's
folder; prints nothing when it takes no first-party package. <dest>/unpublished.txt names each first-party dependency whose
checkout version production's Hub doesn't hold yet: a committed lock pins only what a machine can fetch, so it leaves that one out
until the gated publish relocks it on (cli.sh's lock cell locks a copy without it)."""

import platform
import re
import shutil
import sys
import tomllib
import urllib.request
from pathlib import Path

from packaging.requirements import Requirement
from packaging.utils import canonicalize_name
from packaging.version import InvalidVersion, Version

INDEX = "https://tensorhub.com/v1/index/_"

packages, package, dest, runtime = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]), sys.argv[4]
tensorfs = sys.argv[5] if len(sys.argv) > 5 else ""


def hub_sources(folder: Path) -> list[str]:
    sources = tomllib.loads((folder / "pyproject.toml").read_text()).get("tool", {}).get("uv", {}).get("sources", {})
    hub = lambda source: isinstance(source, dict) and source.get("index") == "tensorhub"  # noqa: E731
    return [key for key, source in sources.items() if (any(map(hub, source)) if isinstance(source, list) else hub(source))]


closure: dict[str, list[str]] = {}
todo = [package]
while todo:
    name = todo.pop()
    if name in closure:
        continue
    closure[name] = [key for key in hub_sources(packages / name) if (packages / key / "pyproject.toml").is_file()]
    todo.extend(closure[name])
if not closure[package]:
    sys.exit(0)

shutil.rmtree(dest, ignore_errors=True)
for name, siblings in closure.items():
    shutil.copytree(packages / name, dest / name, ignore=shutil.ignore_patterns(".venv", "__pycache__"))
    project = dest / name / "pyproject.toml"
    text = project.read_text()
    for key in siblings:
        text, n = re.subn(rf"(?m)^{re.escape(key)} = .*$", f'{key} = {{ path = "../{key}" }}', text)
        assert n == 1, (project, key)
    if name == package and (Path(runtime).is_file() or Path(runtime).is_dir()):
        text = re.sub(r"(?m)^\[tool\.uv\.sources\]$", f'[tool.uv.sources]\ncozy-runtime = {{ path = "{Path(runtime).resolve()}" }}', text, count=1)
    if name == package and Path(tensorfs).is_file():
        text = re.sub(r"(?m)^\[tool\.uv\.sources\]$", f'[tool.uv.sources]\ntensorfs = {{ path = "{Path(tensorfs).resolve()}" }}', text, count=1)
        # uv reads a source only for a dependency the project names itself.
        if not re.search(r'(?m)^\s*"tensorfs[\s\[<>=!~;"]', text):
            text, n = re.subn(r"(?m)^dependencies = \[$", 'dependencies = [\n    "tensorfs",', text, count=1)
            assert n == 1, (project, "dependencies")
    if name == package:
        here = f"\"sys_platform == '{sys.platform}' and platform_machine == '{platform.machine()}'\""
        text, n = re.subn(r"(?ms)^environments = \[.*?^\]", f"environments = [{here}]", text, count=1)
        if not n:
            text += f"\n[tool.uv]\nenvironments = [{here}]\n"
    project.write_text(text)


def published(name: str) -> list[Version] | None:
    """Its versions on production's index. One it can't read is taken as published: the lock check then runs and says why."""
    try:
        with urllib.request.urlopen(f"{INDEX}/simple/{name}/", timeout=30) as page:
            html = page.read().decode()
    except OSError as error:
        print(f"stage.py: {name} on {INDEX}: {error}", file=sys.stderr)
        return None
    found = []
    for version in set(re.findall(rf'href="/v1/index/_/{re.escape(name)}/([^/"]+)/', html)):
        try:
            found.append(Version(version))
        except InvalidVersion:
            pass
    return found


wanted = {canonicalize_name(Requirement(r).name) for r in tomllib.loads((packages / package / "pyproject.toml").read_text())["project"].get("dependencies", [])}


def unmet(key: str) -> bool:
    """The checkout's own version of `key` isn't on production's index: what a lock of this checkout takes of it, no machine can
    fetch (orbit's minimax-h3 0.16.0rc5 while production's newest, 0.15.0, caps the RC's Runtime)."""
    versions = published(key)
    own = Version(tomllib.loads((packages / key / "pyproject.toml").read_text())["project"]["version"])
    return versions is not None and own not in versions


unpublished = [key for key in closure[package] if canonicalize_name(key) in wanted and unmet(key)]
(dest / "unpublished.txt").write_text("".join(f"{key}\n" for key in unpublished))
print(dest / package)
