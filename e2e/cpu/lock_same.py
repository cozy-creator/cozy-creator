"""Whether the uv.lock Calcifer made on this suite's Hub leaves the committed one as it is.

    lock_same.py <committed uv.lock> <made uv.lock> [<the suite Hub's origin>]

A lock that takes a package from a Hub index (orbit's minimax-h3) names production's (`https://tensorhub.com/v1/index/`).
Made on the suite's own Hub it names that Hub's origin instead, and each of that index's wheels is that Hub's build of the
cohort's source: those URLs are read at production's origin and those wheels' hashes are not compared. Production holds
released versions only (Paul 11-13: no prereleases for testing), so the committed lock may name an older release of such a
package than the source the suite's Hub built; never a newer one. Every other byte must be the committed lock's. Prints the
first line that differs and exits 1, or exits 0.

    lock_same.py <committed> <made> [<origin>] [--unpublished <name>,...]

A committed lock pins only what a machine can fetch: each file it names at production's index must be one the Hub serves (a
HEAD each, not followed: a redirect to storage is served, a 4xx isn't), and a first-party dependency the Hub doesn't have yet
(`--unpublished`, stage.py's, which the made lock resolved by path) is no committed lock's: its entries, and every package only
it brings in, are left out of the made one before they're compared.
"""

from __future__ import annotations

import re
import sys
import tomllib
import urllib.error
import urllib.request
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


#: At most this many files a lock pins are asked of the Hub, each within its timeout.
ASKED, TIMEOUT = 32, 20
FILE = re.compile(r'url = "(' + re.escape(PRODUCTION) + r'/v1/index/[^"]+)"')


class Unfollowed(urllib.request.HTTPRedirectHandler):
    """A redirect is the Hub's answer: the file is served, from storage, which a HEAD there would be refused."""

    def redirect_request(self, *args: object, **kwargs: object) -> None:
        return None


def unserved(committed: str) -> str:
    """The first file the committed lock pins at production's index that the Hub answers with a 4xx, said; else ''. One it
    can't ask (a network error, a 5xx) is said on stderr and passes: this judges the lock, not the Hub's health."""
    opener = urllib.request.build_opener(Unfollowed)
    for url in sorted(set(FILE.findall(committed)))[:ASKED]:
        try:
            opener.open(urllib.request.Request(url, method="HEAD"), timeout=TIMEOUT).close()
        except urllib.error.HTTPError as answer:
            if 400 <= answer.code < 500:
                return f"it pins {url}, which the Hub answers {answer.code}: a committed lock pins only what a machine can fetch"
            if answer.code >= 500:
                print(f"lock_same.py: {url}: the Hub answered {answer.code}", file=sys.stderr)
        except OSError as error:
            print(f"lock_same.py: {url}: {error}", file=sys.stderr)
    return ""


def norm(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def only_through(made: str, unpublished: set[str]) -> set[str]:
    """Each unpublished dependency and every package of the made lock only it brings in: none reachable from the project
    without passing through one."""
    if not unpublished:
        return set()
    edges: dict[str, set[str]] = {}
    roots: list[str] = []
    for package in tomllib.loads(made).get("package", []):
        name = norm(package["name"])
        needs = edges.setdefault(name, set())
        groups = [package.get("dependencies", []), *package.get("optional-dependencies", {}).values(),
                  *package.get("dev-dependencies", {}).values()]
        needs.update(norm(dependency["name"]) for group in groups for dependency in group)
        if package.get("source", {}).get("editable") == "." or package.get("source", {}).get("virtual") == ".":
            roots.append(name)
    blocked = {norm(name) for name in unpublished}
    if not roots:
        return blocked
    reached: set[str] = set()
    todo = [root for root in roots if root not in blocked]
    while todo:
        name = todo.pop()
        if name in reached or name in blocked:
            continue
        reached.add(name)
        todo.extend(edges.get(name, ()))
    return set(edges) - reached


def without(made: str, names: set[str]) -> str:
    """The made lock less `names`' blocks, and each mention of them in the rest (a dependency, a requirement)."""
    if not names:
        return made
    blocks = made.split("\n[[package]]\n")
    mention = re.compile(r'\{ name = "(?:' + "|".join(map(re.escape, sorted(names))) + r')"(?:, [^{}]*)? \},? ?')
    kept = [blocks[0]]
    for block in blocks[1:]:
        found = re.match(r'name = "([^"]+)"', block)
        if found is not None and norm(found[1]) in names:
            continue
        lines = []
        for line in block.split("\n"):
            left = mention.sub("", line).replace(", ]", "]")
            if left != line and not left.strip(" ,"):
                continue
            lines.append(left)
        kept.append("\n".join(lines))
    return "\n[[package]]\n".join(kept)


def main() -> int:
    flags = sys.argv[1:]
    unpublished: set[str] = set()
    if "--unpublished" in flags:
        at = flags.index("--unpublished")
        unpublished = {name for name in flags[at + 1].split(",") if name}
        del flags[at:at + 2]
    committed, made = (Path(arg).read_text() for arg in flags[0:2])
    origin = flags[2] if len(flags) > 2 else ""
    if said := unserved(committed):
        print(said[:300])
        return 1
    made = without(made, only_through(made, unpublished))
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
