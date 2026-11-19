#!/usr/bin/env python3
"""version-only.py <checkout> <from> <to> [--members <name,...>] [--files <path>...]: exit 0 only when what changed between two
commits is versions alone (Paul 11-18: test only what changed).
Without --files: every changed file of a packages checkout, each a package's pyproject.toml whose one changed line is its
`version = "..."`, or its uv.lock whose only change is its own row's version, to the same value, so a gate's result stands for
the packages a release publishes (promote.sh --packages-ref); prints each package's move (`sdxl 0.13.0rc2 -> 0.13.1rc1`).
With --files: those paths alone, in any repo (verify.sh --unchanged), each read the same less its own version (a pyproject's
[project] version, a uv.lock's own row's, a Cargo.toml's [package] or [workspace.package] one, a Cargo.lock's workspace
members') and less every mention of a --members project (another of the set's components, which verify.sh judges by its own
diff): a requirement, a lock row, a source. A release cut's version lines and its move from the RC's TensorFS to the release's
pass; prints `version only <path>`.
Anything else refuses, naming the file: `refused <path>: why`."""
import os, re, subprocess, sys, tomllib
from typing import Any

type Toml = Any  # type: ignore[explicit-any]  # a pyproject.toml, uv.lock, Cargo.toml or Cargo.lock, read by key

checkout, old_ref, new_ref, *rest = sys.argv[1:]
members: set[str] = set()
if rest[:1] == ['--members']:
    members, rest = {re.sub(r'[-_.]+', '-', m).lower() for m in rest[1].split(',') if m}, rest[2:]
named = rest[1:] if rest[:1] == ['--files'] else None
VERSION = re.compile(r'[-+]version = "[^"]*"\Z')
REQUIREMENT = re.compile(r'\s*([A-Za-z0-9][A-Za-z0-9._-]*)\s*(?:[\[<>=!~;@ ]|\Z)')


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


def judge(path: str) -> str | None:
    """Packages mode: why the change of `path` is more than its own version; None when it is that alone."""
    d, name = os.path.split(path)
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
    a, b = list(old.get('package', [])), list(new.get('package', []))
    project = (show(new_ref, os.path.join(d, 'pyproject.toml')) or {}).get('project', {})
    own = [i for i, p in enumerate(b) if norm(p.get('name')) == norm(project.get('name'))]
    if len(a) != len(b) or {**old, 'package': None} != {**new, 'package': None} or len(own) != 1 \
            or any(x != y for i, (x, y) in enumerate(zip(a, b)) if i != own[0]) \
            or {**a[own[0]], 'version': None} != {**b[own[0]], 'version': None} or b[own[0]].get('version') != project.get('version'):
        return f"more than {project.get('name')}'s own row's version changed"
    return None


def mentions(x: Toml) -> bool:
    """Whether `x` is a member's requirement string, or a lock row or dependency named it."""
    if isinstance(x, str):
        m = REQUIREMENT.match(x)
        return m is not None and norm(m.group(1)) in members
    return isinstance(x, dict) and norm(x.get('name')) in members


def unmentioned(t: Toml) -> Toml:
    """`t` less every mention of a member (`mentions`, and a key it names: a source), and less the tables and lists that
    leaves empty."""
    if isinstance(t, dict):
        kept = {k: unmentioned(v) for k, v in t.items() if norm(k) not in members}
        return {k: v for k, v in kept.items() if v not in ({}, []) or t[k] in ({}, [])}
    if isinstance(t, list):
        return [unmentioned(x) for x in t if not mentions(x)]
    return t


def bare(path: str, ref: str) -> Toml:
    """`path` at `ref` less its own version and the members' mentions; None when unreadable."""
    d, name = os.path.split(path)
    t = show(ref, path)
    if t is None:
        return None
    if name == 'pyproject.toml':
        t = {**t, 'project': {**t.get('project', {}), 'version': None}}
    elif name == 'Cargo.toml':
        workspace = dict(t.get('workspace') or {})
        t = {**t, 'package': {**(t.get('package') or {}), 'version': None},
             'workspace': {**workspace, 'package': {**(workspace.get('package') or {}), 'version': None}}}
    elif name == 'uv.lock':
        own = norm((show(ref, os.path.join(d, 'pyproject.toml')) or {}).get('project', {}).get('name'))
        t = {**t, 'package': [{**p, 'version': None} if norm(p.get('name')) == own else p for p in t.get('package', [])]}
    else:  # Cargo.lock: a workspace member's row has no source
        t = {**t, 'package': [p if 'source' in p else {**p, 'version': None} for p in t.get('package', [])]}
    return unmentioned(t)


refused, moved = [], {}
for path in named if named is not None else git('diff', '--name-only', old_ref, new_ref).splitlines():
    if named is not None:
        if os.path.basename(path) not in ('pyproject.toml', 'uv.lock', 'Cargo.toml', 'Cargo.lock'):
            refused.append(f'{path}: not a version file')
        elif (a := bare(path, old_ref)) is None or (b := bare(path, new_ref)) is None:
            refused.append(f'{path}: added, removed or unreadable')
        elif a != b:
            refused.append(f"{path}: more than versions changed{' (and ' + ', '.join(sorted(members)) + ')' if members else ''}")
        else:
            print(f'version only {path}')
    elif path.count('/') != 1 or not path.endswith(('/pyproject.toml', '/uv.lock')):
        refused.append(f'{path}: not a package version')
    elif why := judge(path):
        refused.append(f'{path}: {why}')
    elif path.endswith('/pyproject.toml'):
        moved[path.split('/')[0]] = tuple((show(ref, path) or {}).get('project', {}).get('version') for ref in (old_ref, new_ref))
for pkg, (a, b) in sorted(moved.items()):
    print(f'{pkg} {a} -> {b}')
for why in refused:
    print(f'refused {why}')
sys.exit(1 if refused or (named is None and not moved) else 0)
