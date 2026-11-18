#!/usr/bin/env python3
"""version-only.py <packages checkout> <gated commit> <release commit>: exit 0 only when the packages a release publishes differ
from the ones its gate ran by their versions alone, so the gate's result stands for them (Paul 11-18: test only what changed).
Every changed file is a package's pyproject.toml whose one changed line is its `version = "..."`, or that package's uv.lock
whose only change is its own [[package]] row's version, to the same value. Anything else refuses, naming the file.
Prints each package's move (`sdxl 0.13.0rc2 -> 0.13.1rc1`), or why not."""
import re, subprocess, sys, tomllib

checkout, gated, release = sys.argv[1:4]
VERSION = re.compile(r'[-+]version = "[^"]*"\Z')


def git(*args: str) -> str:
    return subprocess.run(['git', '-C', checkout, *args], capture_output=True, text=True, check=True).stdout


def show(ref: str, path: str) -> dict | None:
    try:
        return tomllib.loads(git('show', f'{ref}:{path}'))
    except (subprocess.CalledProcessError, tomllib.TOMLDecodeError):
        return None


def lines(path: str) -> list[str]:
    return [l for l in git('diff', '-U0', gated, release, '--', path).splitlines() if l[:1] in '+-' and not l.startswith(('+++', '---'))]


refused, moved = [], {}
for path in git('diff', '--name-only', gated, release).splitlines():
    pkg, _, name = path.partition('/')
    if name not in ('pyproject.toml', 'uv.lock') or '/' in name:
        refused.append(f'{path}: not a package version')
        continue
    old, new = show(gated, path), show(release, path)
    if old is None or new is None:
        refused.append(f'{path}: added, removed or unreadable')
        continue
    if not all(VERSION.match(l) for l in lines(path)):
        refused.append(f'{path}: a line other than a version changed')
        continue
    project = (show(release, f'{pkg}/pyproject.toml') or {}).get('project', {})
    if name == 'pyproject.toml':
        a, b = old.get('project', {}), new.get('project', {})
        if {**a, 'version': None} != {**b, 'version': None} or {**old, 'project': None} != {**new, 'project': None}:
            refused.append(f'{path}: more than [project] version changed')
            continue
        moved[pkg] = (a.get('version'), b.get('version'))
    else:
        rows = lambda doc: [p for p in doc.get('package', [])]
        a, b = rows(old), rows(new)
        same = len(a) == len(b) and {**old, 'package': None} == {**new, 'package': None}
        norm = lambda n: re.sub(r'[-_.]+', '-', str(n)).lower()  # a lock names packages normalized (PEP 503)
        own = [i for i, p in enumerate(b) if norm(p.get('name')) == norm(project.get('name'))]
        if not same or len(own) != 1 or any(x != y for i, (x, y) in enumerate(zip(a, b)) if i != own[0]) \
                or {**a[own[0]], 'version': None} != {**b[own[0]], 'version': None} or b[own[0]].get('version') != project.get('version'):
            refused.append(f"{path}: more than {project.get('name')}'s own row's version changed")
            continue
for pkg, (a, b) in sorted(moved.items()):
    print(f'{pkg} {a} -> {b}')
for why in refused:
    print(f'refused {why}')
sys.exit(1 if refused or not moved else 0)
