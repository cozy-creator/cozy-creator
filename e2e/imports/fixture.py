"""Stage and verify only fixture-owned Hub dependency locks for the imports suite."""

import argparse
from collections.abc import Iterator
import hashlib
import json
from pathlib import Path
import re
import shutil
import tomllib
from typing import NotRequired, TypedDict, cast
from urllib.parse import urlsplit

# Standard-library TOML/JSON documents at this fixture's file boundary.
type Document = dict[str, object]


class Dependency(TypedDict):
    directory: str
    name: str
    version: str


class Target(TypedDict):
    directory: str
    name: str
    dependencies: list[str]


class Verified(TypedDict):
    package: str
    dependency: str
    version: str
    registry: str
    wheels: list[str]
    lock_sha256: str


class Manifest(TypedDict):
    source: str
    staged: str
    original_locks: dict[str, str]
    original_projects: dict[str, str]
    dependencies: list[Dependency]
    relock: list[Target]
    pre_uv_guard: NotRequired[str]
    original_guard_refusal: NotRequired[str]
    verified: NotRequired[list[Verified]]
    authoritative_checkout_unchanged: NotRequired[bool]
    unaffected_locks_unchanged: NotRequired[bool]


def table(value: object) -> Document:
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError("expected a TOML table")
    return cast(Document, value)


def records(value: object) -> list[Document]:
    if value is None:
        return []
    if not isinstance(value, list):
        raise ValueError("expected a TOML table array")
    return [table(item) for item in value]


def text(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("expected a TOML string")
    return value


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def strings(value: object) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def production(value: str) -> bool:
    # A PEP508 dependency can contain its URL after `name @`, rather than start with it.
    for url in re.findall(r"https?://[^\s<>\"']+", value):
        host = urlsplit(url).hostname
        if host == "tensorhub.com" or (host and host.endswith(".tensorhub.com")):
            return True
    return False


def hub_registry(value: str) -> bool:
    return value.startswith(("https://", "http://")) and "/v1/index/" in urlsplit(value).path


def name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).lower()


def load(path: Path) -> Document:
    return cast(Document, tomllib.loads(path.read_text()))


def snapshot(root: Path, pattern: str) -> dict[str, str]:
    return {str(path.relative_to(root)): digest(path) for path in sorted(root.glob(pattern))}


def resolver_values(project: Document) -> Iterator[str]:
    uv = table(table(project.get("tool")).get("uv"))
    yield from strings(uv.get("index", []))
    yield from strings(uv.get("sources", {}))
    yield from strings(table(project.get("project")).get("dependencies", []))
    yield from strings(table(project.get("project")).get("optional-dependencies", {}))
    yield from strings(project.get("dependency-groups", {}))


def guard(root: Path, origin: str | None = None) -> None:
    for path in root.rglob("uv.lock"):
        for value in strings(load(path)):
            if production(value):
                raise ValueError(f"production URL remains in staged {path.relative_to(root)}")
            if hub_registry(value) and (origin is None or not value.startswith(origin.rstrip("/") + "/v1/index/")):
                raise ValueError(f"unbound Hub URL remains in staged {path.relative_to(root)}")
    for path in root.glob("*/pyproject.toml"):
        if any(production(value) for value in resolver_values(load(path))):
            raise ValueError(f"production resolver URL in staged {path.relative_to(root)}")


def prepare(source: Path, staged: Path, report: Path) -> None:
    source = source.resolve(strict=True)
    projects = {path.parent.name: load(path) for path in sorted(source.glob("*/pyproject.toml"))}
    by_name = {name(text(table(project["project"])["name"])): directory for directory, project in projects.items()}
    targets: list[Target] = []
    dependencies: dict[str, Dependency] = {}
    for directory, project in projects.items():
        sources = table(table(table(project.get("tool")).get("uv")).get("sources"))
        hub_names = {name(key) for key, value in sources.items()
                     if isinstance(value, dict) and text(table(value).get("index", "")).startswith("tensorhub")}
        lock_path = source / directory / "uv.lock"
        locked = records(load(lock_path).get("package")) if lock_path.exists() else []
        for package in locked:
            registry = text(table(package.get("source")).get("registry", ""))
            if hub_registry(registry):
                if urlsplit(registry).path.rstrip("/") != "/v1/index/_/simple":
                    raise ValueError(f"{directory} needs a non-root Hub dependency, outside this fixture")
                hub_names.add(name(text(package["name"])))
        if not hub_names:
            continue
        for dependency in sorted(hub_names):
            if dependency not in by_name:
                raise ValueError(f"{directory}: Hub dependency {dependency} has no cohort checkout")
            folder = by_name[dependency]
            version = text(table(projects[folder]["project"])["version"])
            versions = {text(item["version"]) for item in locked if name(text(item["name"])) == dependency}
            if versions and versions != {version}:
                raise ValueError(f"{directory}: locked {dependency} {versions} differs from cohort {version}")
            dependencies[dependency] = {"directory": folder, "name": dependency, "version": version}
        targets.append({"directory": directory, "name": text(table(project["project"])["name"]), "dependencies": sorted(hub_names)})
    if any(item["directory"] in {target["directory"] for target in targets} for item in dependencies.values()):
        raise ValueError("a fixture dependency itself needs a Hub lock; publish ordering must be explicit")
    manifest: Manifest = {"source": str(source), "staged": str(staged.resolve()),
                "original_locks": snapshot(source, "**/uv.lock"),
                "original_projects": snapshot(source, "*/pyproject.toml"),
                "dependencies": list(dependencies.values()), "relock": targets}
    shutil.copytree(source, staged, ignore=shutil.ignore_patterns(".git", ".venv", "__pycache__"))
    try:
        guard(staged)
    except ValueError as error:
        # Record that the actual incoming lock is rejected without invoking a resolver.
        manifest["original_guard_refusal"] = str(error)
    for target in targets:
        # Never hand the original Hub wheel/index URLs to uv, even during lock regeneration.
        (staged / target["directory"] / "uv.lock").unlink(missing_ok=True)
    guard(staged)
    manifest["pre_uv_guard"] = "passed: every remaining staged lock/resolver has no production Hub URL"
    report.write_text(json.dumps(manifest, indent=2) + "\n")


def verify(report: Path, origin: str) -> None:
    manifest = cast(Manifest, json.loads(report.read_text()))
    source, staged = Path(manifest["source"]), Path(manifest["staged"])
    if snapshot(source, "**/uv.lock") != manifest["original_locks"] or snapshot(source, "*/pyproject.toml") != manifest["original_projects"]:
        raise ValueError("the authoritative packages checkout changed during fixture staging")
    guard(staged, origin)
    changed = {target["directory"] + "/uv.lock" for target in manifest["relock"]}
    for path, expected in manifest["original_locks"].items():
        if path not in changed and digest(staged / path) != expected:
            raise ValueError(f"unaffected staged lock changed: {path}")
    if snapshot(staged, "*/pyproject.toml") != manifest["original_projects"]:
        raise ValueError("staged package source metadata changed")
    expected = {item["name"]: item["version"] for item in manifest["dependencies"]}
    actual: list[Verified] = []
    for target in manifest["relock"]:
        path = staged / target["directory"] / "uv.lock"
        packages = records(load(path).get("package"))
        for dependency in target["dependencies"]:
            rows = [item for item in packages if name(text(item["name"])) == dependency]
            if len(rows) != 1 or rows[0]["version"] != expected[dependency]:
                raise ValueError(f"{target['directory']}: fixture {dependency} version was not locked")
            row = rows[0]
            registry = text(table(row.get("source")).get("registry", ""))
            wheels = [text(wheel["url"]) for wheel in records(row.get("wheels"))]
            if registry.rstrip("/") != origin.rstrip("/") + "/v1/index/_/simple" or not wheels:
                raise ValueError(f"{target['directory']}: dependency does not come from the fixture root index")
            if any(not url.startswith(origin.rstrip("/") + "/v1/index/_/") for url in wheels):
                raise ValueError(f"{target['directory']}: dependency wheel is outside the fixture")
            actual.append({"package": target["directory"], "dependency": dependency,
                           "version": text(row["version"]), "registry": registry, "wheels": wheels,
                           "lock_sha256": digest(path)})
    manifest.update(verified=actual, authoritative_checkout_unchanged=True, unaffected_locks_unchanged=True)
    report.write_text(json.dumps(manifest, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "verify"))
    parser.add_argument("--source", type=Path)
    parser.add_argument("--staged", type=Path)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--origin")
    args = parser.parse_args()
    try:
        if args.action == "prepare":
            if args.source is None or args.staged is None:
                parser.error("prepare requires --source and --staged")
            prepare(args.source, args.staged, args.report)
        else:
            if not args.origin:
                parser.error("verify requires --origin")
            verify(args.report, args.origin)
    except (ValueError, OSError) as error:
        raise SystemExit(f"imports fixture: {error}") from None


if __name__ == "__main__":
    main()
