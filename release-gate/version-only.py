#!/usr/bin/env python3
"""version-only.py <checkout> <from> <to> [--files <path>...]: exit 0 only when what changed between two commits is versions
alone (Paul 11-18: test only what changed). A file passes when every changed line is a `version = "..."` line and it reads the
same otherwise: a pyproject.toml's [project] version, its uv.lock's own [[package]] row's version (to the pyproject's), a
Cargo.toml's [package] or [workspace.package] version, a Cargo.lock's version of a workspace member (a row with no source).
Without --files: every changed file of a packages checkout, each a package's pyproject.toml or uv.lock, so a gate's result stands
for the packages a release publishes (promote.sh --packages-ref); prints each package's move (`sdxl 0.13.0rc2 -> 0.13.1rc1`).
With --files: those paths alone, in any repo (verify.sh --unchanged: a release cut's version lines); prints `version only <path>`.
Anything else refuses, naming the file: `refused <path>: why`."""
import os, re, subprocess, sys, tomllib
from typing import Any

type Toml = Any  # type: ignore[explicit-any]  # a pyproject.toml, uv.lock, Cargo.toml or Cargo.lock, read by key

checkout, old_ref, new_ref = sys.argv[1:4]
named = sys.argv[5:] if sys.argv[4:5] == ['--files'] else None
VERSION = re.compile(r'[-+]version = "[^"]*"\Z')


def git(*args: str) -> str:
    return subprocess.run(['git', '-C', checkout, *args], capture_output=True, text=True, check=True).stdout


def norm(name: object) -> str:
    """A package name as a lock writes it (PEP 503)."""
    return re.sub(r'[-_.]+', '-', str(name)).lower()


def show(ref: str, path: str) -> dict[str, Toml] | None:
    try:
        return tomllib.loads(git('show', f'{ref}:{path}'))
    except (subprocess.CalledProcessError, tomllib.TOMLDecodeError):
        return None


def lines(path: str) -> list[str]:
    return [l for l in git('diff', '-U0', old_ref, new_ref, '--', path).splitlines() if l[:1] in '+-' and not l.startswith(('+++', '---'))]


def cargo(t: dict[str, Toml]) -> dict[str, Toml]:
    """A Cargo.toml less its [package] and [workspace.package] versions."""
    workspace = dict(t.get('workspace') or {})
    members = {**(workspace.get('package') or {}), 'version': None}
    return {**t, 'package': {**(t.get('package') or {}), 'version': None}, 'workspace': {**workspace, 'package': members}}


def judge(path: str) -> str | None:
    """Why the change of `path` is more than versions; None when it is versions alone."""
    d, name = os.path.split(path)
    if name not in ('pyproject.toml', 'uv.lock', 'Cargo.toml', 'Cargo.lock'):
        return 'not a version file'
    old, new = show(old_ref, path), show(new_ref, path)
    if old is None or new is None:
        return 'added, removed or unreadable'
    if not all(VERSION.match(l) for l in lines(path)):
        return 'a line other than a version changed'
    if name == 'pyproject.toml':
        a, b = old.get('project', {}), new.get('project', {})
        if {**a, 'version': None} != {**b, 'version': None} or {**old, 'project': None} != {**new, 'project': None}:
            return 'more than [project] version changed'
        return None
    if name == 'Cargo.toml':
        return None if cargo(old) == cargo(new) else 'more than [package] or [workspace.package] version changed'
    a, b = list(old.get('package', [])), list(new.get('package', []))
    if len(a) != len(b) or {**old, 'package': None} != {**new, 'package': None}:
        return 'its rows changed'
    if name == 'Cargo.lock':
        if any('source' in x or 'source' in y or {**x, 'version': None} != {**y, 'version': None} for x, y in zip(a, b) if x != y):
            return "more than a workspace member's version changed"
        return None
    project = (show(new_ref, os.path.join(d, 'pyproject.toml')) or {}).get('project', {})
    own = [i for i, p in enumerate(b) if norm(p.get('name')) == norm(project.get('name'))]
    if len(own) != 1 or any(x != y for i, (x, y) in enumerate(zip(a, b)) if i != own[0]) \
            or {**a[own[0]], 'version': None} != {**b[own[0]], 'version': None} or b[own[0]].get('version') != project.get('version'):
        return f"more than {project.get('name')}'s own row's version changed"
    return None


refused, moved = [], {}
for path in named if named is not None else git('diff', '--name-only', old_ref, new_ref).splitlines():
    if named is None and (path.count('/') != 1 or not path.endswith(('/pyproject.toml', '/uv.lock'))):
        refused.append(f'{path}: not a package version')
    elif why := judge(path):
        refused.append(f'{path}: {why}')
    elif named is not None:
        print(f'version only {path}')
    elif path.endswith('/pyproject.toml'):
        moved[path.split('/')[0]] = tuple((show(ref, path) or {}).get('project', {}).get('version') for ref in (old_ref, new_ref))
for pkg, (a, b) in sorted(moved.items()):
    print(f'{pkg} {a} -> {b}')
for why in refused:
    print(f'refused {why}')
sys.exit(1 if refused or (named is None and not moved) else 0)
