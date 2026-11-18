"""GitHub's Latest cozy and the newest release of the line before it, each with the Runtime and TensorFS it ships (its notes'
first line: "cozy X, Runtime Y and TensorFS Z"): what cozy updates from (cli/selfupdate.sh, candidates/build.sh's cozy-before),
read at each run so a promotion never leaves it behind. Prints shell assignments: LATEST LATEST_RUNTIME LATEST_TENSORFS PREVIOUS
PREVIOUS_RUNTIME PREVIOUS_TENSORFS. $GH_TOKEN, when set, authenticates the reads."""

import json
import os
import re
import sys
import urllib.request

API = "https://api.github.com/repos/cozy-creator/cozy-creator/releases"
SHIPS = re.compile(r"^cozy (\S+), Runtime (\S+) and TensorFS (\S+?)[:.]?(\s|$)")


def read(url: str) -> object:
    headers = {"Accept": "application/vnd.github+json"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = f"Bearer {os.environ['GH_TOKEN']}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as answer:
        return json.load(answer)


def ships(release: dict) -> tuple[str, str, str]:
    found = SHIPS.match((release.get("body") or "").strip())
    if found is None:
        sys.exit(f"releases.py: {release['tag_name']}'s notes don't start \"cozy X, Runtime Y and TensorFS Z\"")
    return found.group(1), found.group(2), found.group(3)


def version(tag: str) -> tuple[int, ...] | None:
    found = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", tag)
    return tuple(int(part) for part in found.groups()) if found else None


latest = read(f"{API}/latest")
assert isinstance(latest, dict)
cozy, runtime, tensorfs = ships(latest)
line = version(latest["tag_name"])
if line is None:
    sys.exit(f"releases.py: GitHub's Latest is {latest['tag_name']}, not a cozy release")
releases = read(f"{API}?per_page=100")
assert isinstance(releases, list)
before = [r for r in releases if not r["draft"] and not r["prerelease"] and (v := version(r["tag_name"])) and v[:2] == (line[0], line[1] - 1)]
if not before:
    sys.exit(f"releases.py: no release of {line[0]}.{line[1] - 1}, the line before the Latest {cozy}")
previous = max(before, key=lambda r: version(r["tag_name"]) or ())
was = ships(previous)
print(f"LATEST={cozy} LATEST_RUNTIME={runtime} LATEST_TENSORFS={tensorfs} PREVIOUS={was[0]} PREVIOUS_RUNTIME={was[1]} "
      f"PREVIOUS_TENSORFS={was[2]}")
