#!/usr/bin/env python3
"""Suite `checkpoints` (xs-183): every published checkpoint against what its tool reads, from headers alone.

Walks every public model's and LoRA's newest release, and the release each package's model slot names by default (as a
run takes it: its series' newest fix), reads each lane through `cozy model show --json`, and holds it to every entry of
requirements.toml that matches it. Prints one line per gap (`GAP <model>@<release> <lane>: <item>  [<what reads it>]`)
and exits 1 when there is any. No GPU, no rental, no weights:

    release-gate/suites/checkpoints/walk.py [--cli cozy] [--out DIR] [--only org/name,...]
"""
import argparse
import json
import pathlib
import subprocess
import sys
import tomllib
from concurrent.futures import ThreadPoolExecutor
from typing import Any

HERE = pathlib.Path(__file__).resolve().parent
type Json = Any  # type: ignore[explicit-any]  # `cozy --json` answers and requirements.toml, read by key


class Refused(Exception):
    pass


def cozy(cli: str, *args: str) -> Json:
    """`cozy --json <args>`'s answer, or Refused with its message."""
    done = subprocess.run([cli, "--json", *args], capture_output=True, text=True, timeout=900)
    try:
        answer = json.loads(done.stdout)
    except ValueError:
        answer = None
    if done.returncode or answer is None or "error" in (answer if isinstance(answer, dict) else {}):
        why = (answer or {}).get("error", {}).get("message") if isinstance(answer, dict) else None
        raise Refused(why or f"exit {done.returncode}: {(done.stdout + done.stderr).strip()[-300:]}")
    return answer


def pages(cli: str, *args: str, key: str) -> list[Json]:
    rows: list[Json] = []
    offset = 0
    while True:
        page = cozy(cli, *args, "--limit", "100", "--offset", str(offset))
        rows += page[key]
        offset += len(page[key])
        if not page[key] or offset >= page.get("total", 0):
            return rows


def defaults(cli: str, jobs: int) -> dict[str, list[str]]:
    """Each Hub reference a package's model slot loads by default: {ref: ["<package>@<release> <slot path>", ...]}."""
    packages = [p for p in pages(cli, "package", "ls", key="packages") if p.get("latest_release")]

    def slots(package: Json) -> list[tuple[str, str]]:
        named = f"{package['package']}@{package['latest_release']}"
        interface = cozy(cli, "--full", "run", named, "--describe").get("interface") or {}
        return [(slot["default"], f"{named} {slot.get('path', '')}") for entry in interface.get("entrypoints", [])
                for slot in entry.get("models") or [] if slot.get("default")]

    found: dict[str, list[str]] = {}
    with ThreadPoolExecutor(jobs) as pool:
        for held in pool.map(slots, packages):
            for ref, where in held:
                if "://" not in ref and not ref.startswith("local/"):
                    found.setdefault(ref, []).append(where)
    return found


def files_of(component: Json) -> set[str]:
    """Its run data, its config's file among them."""
    held = set(component["assets"])
    if component["config_keys"]:
        held.add(component.get("config_file") or "config.json")
    return held


def component_gaps(name: str, spec: Json, components: dict[str, Json]) -> list[str]:
    held = components.get(name)
    if held is None:
        return [f"{name} (no such component)"]
    gaps = []
    keys = spec.get("keys", [])
    if (spec.get("config", True) or keys) and not held["config_keys"]:
        reads = f"; reads {', '.join(keys)}" if keys else ""
        gaps.append(f"{name}/{held.get('config_file') or 'config.json'} (no config{reads})")
    else:
        gaps += [f"{name} config key {key}" for key in keys if key not in held["config_keys"]]
    have = files_of(held)
    for wanted in spec.get("files", []):
        options = [option.split("+") for option in wanted.split("|")]
        if any(all(path in have for path in option) for option in options):
            continue
        missing = "+".join(path for path in options[0] if path not in have)
        others = " or ".join("+".join(option) for option in options[1:])
        gaps.append(f"{name}/{missing}" + (f" (or {others})" if others else ""))
    if spec.get("tensors") and not held["tensors"]:
        gaps.append(f"{name} (no weights)")
    return gaps


def lora_gaps(lane: Json) -> list[str]:
    adapters = lane["records"].get("adapter")
    adapters = adapters if isinstance(adapters, dict) else {}
    loras = [c for c in lane["components"] if c.get("lora")]
    if not loras:
        return ["no LoRA factors (A/B pairs)"]
    gaps = []
    for held in loras:
        record = adapters.get(held["name"])
        if not (isinstance(record, dict) and record.get("family") and record.get("component")):
            gaps.append(f"{held['name']}: adapter record (its family and component)")
        lora = held["lora"]
        if lora.get("lora_alpha") is None and lora["alphas"] < lora["modules"]:
            gaps.append(f"{held['name']}: α for {lora['modules'] - lora['alphas']} of {lora['modules']} modules")
    return gaps


def entries(requirements: list[Json], model: str, label: str | None, lane: Json) -> list[Json]:
    """The entries that hold `lane`: those naming the model, else those of its families and those of its kind alone."""
    named = [e for e in requirements if model in e.get("models", [])]
    if named:
        return named
    families = {lane.get("family"), label} - {None}
    def kind(entry: Json) -> bool:
        return "kind" not in entry or entry["kind"] == lane.get("kind")
    by_family = [e for e in requirements if families & set(e.get("families", [])) and kind(e)]
    by_kind = [e for e in requirements if not e.get("models") and not e.get("families") and kind(e)]
    return by_family + by_kind


def check(requirements: list[Json], shown: Json) -> tuple[list[tuple[str, str, str]], list[tuple[str, str]]]:
    """(gaps, unchecked) of one release: gaps as (lane, item, reads)."""
    gaps: list[tuple[str, str, str]] = []
    unchecked: list[tuple[str, str]] = []
    for lane in shown["lanes"]:
        held = entries(requirements, shown["model"], shown.get("family"), lane)
        if not held:
            unchecked.append((lane["lane"], f"no entry for family {lane.get('family') or shown.get('family') or '-'}, kind {lane.get('kind') or '-'}"))
        components = {c["name"]: c for c in lane["components"]}
        seen: set[tuple[str, str]] = set()
        for entry in held:
            items = [g for name, spec in entry.get("components", {}).items() for g in component_gaps(name, spec, components)]
            items += lora_gaps(lane) if entry.get("lora") else []
            for item in items:
                if (lane["lane"], item) not in seen:
                    seen.add((lane["lane"], item))
                    gaps.append((lane["lane"], item, entry.get("reads", "")))
    return gaps, unchecked


def main() -> int:
    ask = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ask.add_argument("--cli", default="cozy")
    ask.add_argument("--requirements", default=str(HERE / "requirements.toml"))
    ask.add_argument("--out", help="a folder for findings.json and each release's `model show --json` (shown/)")
    ask.add_argument("--only", default="", help="org/name,... to walk alone (their newest releases)")
    ask.add_argument("--jobs", type=int, default=4)
    args = ask.parse_args()
    requirements: list[Json] = tomllib.loads(pathlib.Path(args.requirements).read_text())["checkpoint"]
    only = {name.strip() for name in args.only.split(",") if name.strip()}

    named: dict[str, list[str]] = {}  # "org/name@release" -> why it is walked
    errors: list[tuple[str, str]] = []
    for noun in ("model", "lora"):
        for row in pages(args.cli, noun, "ls", key="models"):
            if not row.get("private") and row.get("release") and (not only or row["model"] in only):
                named.setdefault(f"{row['model']}@{row['release']}", []).append(f"newest {noun}")
    slots = {} if only else defaults(args.cli, args.jobs)
    for ref, where in slots.items():
        named.setdefault(ref, []).extend(f"default of {w}" for w in where)

    def show(ref: str) -> tuple[str, Json | None, str | None]:
        try:
            return ref, cozy(args.cli, "model", "show", ref), None
        except Refused as why:
            return ref, None, str(why)

    shown: dict[str, Json] = {}
    forward: dict[str, str] = {}
    asked = set(named)
    with ThreadPoolExecutor(args.jobs) as pool:
        todo = list(named)
        while todo:
            later: list[str] = []
            for ref, answer, why in pool.map(show, todo):
                if answer is None:
                    errors.append((ref, why or "no answer"))
                    continue
                key = f"{answer['model']}@{answer['release']}"
                # A run naming an older release takes its series' newest fix: that, not the one named, is what loads.
                if answer.get("resolves_to"):
                    newer = f"{answer['model']}@{answer['resolves_to']}"
                    forward[key] = newer
                    named.setdefault(newer, []).extend(f"{w}, named @{answer['release']}" for w in named.get(ref, []))
                    if newer not in asked:
                        asked.add(newer)
                        later.append(newer)
                    continue
                shown.setdefault(key, answer)
                if ref != key:
                    named.setdefault(key, []).extend(named.get(ref, []))
            todo = later

    releases: dict[str, Json] = {}
    found_gaps: list[Json] = []
    no_entry: list[Json] = []
    for key in sorted(shown):
        answer = shown[key]
        gaps, unchecked = check(requirements, answer)
        held = sorted({e.get("reads", "") for lane in answer["lanes"] for e in entries(requirements, answer["model"], answer.get("family"), lane)})
        releases[key] = {"why": sorted(set(named.get(key, []))), "lanes": [lane["lane"] for lane in answer["lanes"]], "held_to": held}
        found_gaps += [{"release": key, "lane": lane, "item": item, "reads": reads} for lane, item, reads in gaps]
        no_entry += [{"release": key, "lane": lane, "why": why} for lane, why in unchecked]
    findings = {"releases": releases, "resolved_forward": forward, "gaps": found_gaps, "unchecked": no_entry,
                "errors": [{"ref": r, "error": e} for r, e in errors]}
    for gap in found_gaps:
        print(f"GAP {gap['release']} {gap['lane']}: {gap['item']}  [{gap['reads']}]")
    for ref, why in errors:
        print(f"UNREAD {ref}: {why}")
    for lane in no_entry:
        print(f"UNCHECKED {lane['release']} {lane['lane']}: {lane['why']}")
    lanes = sum(len(r["lanes"]) for r in releases.values())
    short = len({g["release"] for g in found_gaps})
    print(f"checkpoints: {len(shown)} releases, {lanes} lanes; {len(found_gaps)} gaps in {short} releases, "
          f"{len(errors)} unread, {len(no_entry)} lanes no entry covers")
    if args.out:
        out = pathlib.Path(args.out)
        (out / "shown").mkdir(parents=True, exist_ok=True)
        (out / "findings.json").write_text(json.dumps(findings, indent=1) + "\n")
        for key, answer in shown.items():
            (out / "shown" / (key.replace("/", "__") + ".json")).write_text(json.dumps(answer, indent=1) + "\n")
    return 1 if found_gaps or errors else 0


if __name__ == "__main__":
    sys.exit(main())
