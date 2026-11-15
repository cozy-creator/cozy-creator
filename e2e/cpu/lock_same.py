"""Whether the uv.lock Calcifer made on this suite's Hub leaves the committed one as it is.

    lock_same.py <committed uv.lock> <made uv.lock> [<the suite Hub's origin>]

A lock that takes a package from a Hub index (orbit's minimax-h3) names production's (`https://tensorhub.com/v1/index/`).
Made on the suite's own Hub it names that Hub's origin instead, and each of that index's wheels is that Hub's build of the
cohort's source: those URLs are read at production's origin and those wheels' hashes are not compared. Production holds
released versions only (Paul 11-13: no prereleases for testing), so the committed lock may name an older release of such a
package than the source the suite's Hub built; never a newer one. Every other byte must be the committed lock's. Prints the
first line that differs and exits 1, or exits 0.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

PRODUCTION = "https://tensorhub.com"
#: A Hub index's file and its hash, as uv writes it: `{ url = "<hub>/v1/index/...", hash = "sha256:..." }`.
HUB_FILE = re.compile(r'(url = "' + re.escape(PRODUCTION) + r'/v1/index/[^"]*", hash = )"sha256:[0-9a-f]{64}"')
#: A package block's name, version and registry, as uv writes them.
BLOCK = re.compile(r'name = "([^"]+)"\nversion = "([^"]+)"\nsource = \{ registry = "' + re.escape(PRODUCTION) + r'/v1/index/')
#: A PEP 440 version of the forms uv locks: a release, an a/b/rc pre-release, .postN, .devN.
VERSION = re.compile(r"(\d+(?:\.\d+)*)(?:(a|b|rc)(\d+))?(?:\.post(\d+))?(?:\.dev(\d+))?")


def key(version: str) -> tuple[tuple[int, ...], tuple[int, int], int, float]:
    """`version`'s PEP 440 order."""
    found = VERSION.fullmatch(version)
    if found is None:
        raise ValueError(f"{version} is not a version this check orders")
    release, kind, pre, post, dev = found.groups()
    numbers = tuple(int(part) for part in release.split("."))
    numbers += (0,) * (8 - len(numbers))
    stage = (("a", "b", "rc").index(kind), int(pre)) if kind else (-1, 0) if dev and not post else (3, 0)
    return numbers, stage, int(post) if post else -1, int(dev) if dev else float("inf")


#: The open RC's Runtime build a lock binds (`0.47.0.dev2287+g1a53ab3c`, its wheel on the `rc-<x>` release): each Runtime merge
#: replaces it and nothing relocks (RELEASING.md), so any build of the minor is the same lock.
RC_BUILD = re.compile(r"(\d+\.\d+\.\d+)\.dev\d+\+g[0-9a-f]+")
RC_FILE = re.compile(r'(url = "https://github\.com/cozy-creator/cozy-runtime/releases/download/rc-[^"]*"(?:, hash = )?)("sha256:[0-9a-f]{64}")?')


def normal(text: str, origin: str) -> str:
    if origin:
        text = text.replace(origin.rstrip("/") + "/v1/index/", PRODUCTION + "/v1/index/")
    text = RC_FILE.sub(lambda m: m.group(1) + ('"sha256:*"' if m.group(2) else ""), text)
    text = RC_BUILD.sub(r"\1.devRC", text)
    return HUB_FILE.sub(r'\1"sha256:*"', text)


def cohort(want: str, got: str) -> str:
    """`want` with each Hub index package at the version the suite's Hub built (`got`), where it names an older one."""
    built = dict(BLOCK.findall(got))
    blocks = want.split("\n[[package]]\n")
    for at, block in enumerate(blocks):
        found = BLOCK.search(block)
        if found is None or found[1] not in built or found[2] == built[found[1]]:
            continue
        if key(found[2]) > key(built[found[1]]):
            raise ValueError(f"the committed lock's {found[1]} {found[2]} is newer than its source, {built[found[1]]}")
        blocks[at] = block.replace(found[2], built[found[1]])
    return "\n[[package]]\n".join(blocks)


def main() -> int:
    committed, made = (Path(arg).read_text() for arg in sys.argv[1:3])
    origin = sys.argv[3] if len(sys.argv) > 3 else ""
    got = normal(made, origin)
    try:
        want = cohort(normal(committed, origin), got)
    except ValueError as error:
        print(str(error)[:300])
        return 1
    if want == got:
        return 0
    lines = list(zip(want.splitlines(), got.splitlines()))
    number = next((i for i, (a, b) in enumerate(lines, 1) if a != b), len(lines) + 1)
    a, b = (text.splitlines()[number - 1] if number <= len(text.splitlines()) else "(end)" for text in (want, got))
    print(f"line {number}: {a.strip()} -> {b.strip()}"[:300])
    return 1


if __name__ == "__main__":
    sys.exit(main())
