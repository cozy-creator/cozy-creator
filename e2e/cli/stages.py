"""A run's stages against the page's words for them (cl-921): `web/app/src/panels/runs/stages.json`, read as the page reads it.

`python stages.py <run.ndjson> [stages.json]`, on a run followed with `cozy --json run`: every part of every stage it went through is
in the map; a count the map calls steps is only where a stage denoises (`phase: working`); its executor's start or load is said before its work;
an install says its steps in words (`…: creating its environment (2 of 5)`); a stage the map counts in bytes
had bytes; while bytes move at a known rate, it has a time left (at least theirs). Prints the run's phases in order, in the page's words, with when each began. Exits 1 on the first list of problems.
"""

import json
import re
import sys
from pathlib import Path
from typing import TypedDict


class Entry(TypedDict, total=False):
    """One entry of the map: `match`, `says`, its `phase`, what a count under it `counts`, whether it is a `scope`."""

    match: str
    says: str
    phase: str
    counts: str
    scope: bool


class Progress(TypedDict, total=False):
    """A `run.progress` event's progress, as far as this reads it."""

    stage: str
    at_ms: int
    bytes_done: int
    bytes_total: int
    bytes_per_s: float | None
    eta_s: int | None

MAP = Path(__file__).resolve().parents[2] / "web/app/src/panels/runs/stages.json"


def compiled(path: Path) -> list[tuple[Entry, list[str], re.Pattern[str]]]:
    """Each entry's `match` as a pattern: literal text, `{name}` any text, `{#name}` digits; the whole part, any case."""
    out = []
    for entry in json.loads(path.read_text())["stages"]:
        names: list[str] = []

        def piece(found: re.Match[str]) -> str:
            if found.group(2) is None:
                return re.escape(found.group(0))
            names.append(found.group(2))
            return r"(\d+)" if found.group(1) else r"(.+?)"

        out.append((entry, names, re.compile("^" + re.sub(r"\{(#?)(\w+)\}|.", piece, entry["match"], flags=re.S) + "$", re.I | re.S)))
    return out


def said(entries: list[tuple[Entry, list[str], re.Pattern[str]]], part: str) -> Entry | None:
    """The first entry `part` matches, its words filled; none: the map doesn't know it."""
    for entry, names, pattern in entries:
        found = pattern.match(part.strip())
        if found:
            words = re.sub(r"\{(\w+)\}", lambda m: found.group(names.index(m.group(1)) + 1) if m.group(1) in names else m.group(0), entry["says"])
            filled = entry.copy()
            filled["says"] = words
            return filled
    return None


def main() -> int:
    run = Path(sys.argv[1])
    entries = compiled(Path(sys.argv[2]) if len(sys.argv) > 2 else MAP)
    problems = [f"stages.json: {e['match']!r} counts steps outside the work" for e, _, _ in entries if e.get("counts") == "step" and e.get("phase") != "working"]
    seen: list[tuple[int, str]] = []
    progress: list[Progress] = []
    for line in run.read_text().splitlines():
        if not line.startswith("{"):
            continue
        event = json.loads(line)
        if event.get("type") == "run.log" and event.get("level") == "stage":
            if not seen or seen[-1][1] != event["text"]:
                seen.append((event.get("at_ms", 0), event["text"]))
        elif event.get("type") == "run.progress" and event.get("progress", {}).get("stage"):
            progress.append(event["progress"])
            if not seen or seen[-1][1] != event["progress"]["stage"]:
                seen.append((event["progress"].get("at_ms", 0), event["progress"]["stage"]))
    if not seen:
        return print(f"{run}: no stages") or 1
    phases = []
    for at, stage in seen:
        parts = [part for part in re.split(r"\s+/\s+", stage.strip()) if part]
        known = [said(entries, part) for part in parts]
        for part, entry in zip(parts, known):
            if entry is None:
                problems.append(f"{part!r} (of {stage!r}) is not in stages.json")
        last = known[-1] if known else None
        phase = last.get("phase") if last else None
        phases.append((at, phase, " · ".join(str(entry["says"]) if entry else part for part, entry in zip(parts, known))))
    installs = [stage for _, stage in seen if re.search(r"^installing .+ on this machine", stage.rsplit(" / ", 1)[-1])]
    if installs and not any(re.search(r": .+ \(\d+ of \d+\)$", stage) for stage in installs):
        problems.append(f"an install that never said its steps in words: {installs}")
    work = next((at for at, (_, phase, _) in enumerate(phases) if phase == "working"), None)
    if work is not None and not any(phase in ("starting", "importing", "loading") for _, phase, _ in phases[:work]):
        problems.append("nothing said between its preparation and its work (no `importing <package>`, `starting the executor`, `loading …`)")
    for each in progress:
        entry = said(entries, re.split(r"\s+/\s+", each["stage"].strip())[-1])
        if entry and entry.get("counts") == "bytes" and entry.get("phase") == "downloading" and not each.get("bytes_total"):
            problems.append(f"{each['stage']!r} counts bytes and said none")
        moving = (each.get("bytes_per_s") or 0) >= 1 and each.get("bytes_done", 0) < each.get("bytes_total", 0)
        if moving and each.get("eta_s") is None:
            problems.append(f"{each['stage']!r}: bytes move at a known rate and it has no time left")
    first = phases[0][0] or 0
    for at, phase, words in phases:
        print(f"  {(at - first) / 1000 if at else 0:7.1f}s  {phase or '?':<11}  {words}")
    for problem in problems:
        print(f"PROBLEM {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
