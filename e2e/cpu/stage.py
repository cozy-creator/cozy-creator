"""stage.py <packages> <package> <dest> <runtime>: the package as an RC machine reads it from a packages checkout (xs-195,
Calcifer's `checkout::stage`). Each `{ index = "tensorhub" }` source naming a package the checkout holds becomes a path to
that folder, copied beside it, and so on through what those take. The Runtime it is read with is `<runtime>` when that is
a wheel or a source folder, and it is locked for this computer alone, as a machine's lock is. Prints the staged package's
folder; prints nothing when it takes no first-party package."""

import platform
import re
import shutil
import sys
import tomllib
from pathlib import Path

packages, package, dest, runtime = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]), sys.argv[4]


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
    if name == package:
        here = f"\"sys_platform == '{sys.platform}' and platform_machine == '{platform.machine()}'\""
        text, n = re.subn(r"(?ms)^environments = \[.*?^\]", f"environments = [{here}]", text, count=1)
        if not n:
            text += f"\n[tool.uv]\nenvironments = [{here}]\n"
    project.write_text(text)
print(dest / package)
