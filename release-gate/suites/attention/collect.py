#!/usr/bin/env python3
"""Turn attention-suite gate directories into the Runtime's measurements/attention.json (xs-171).

    collect.py --runtime <cozy-runtime checkout> [--into FILE] <gate dir>...

Each directory is one pod: its class's record (inventory, every speed set's points, every kernel's quality) replaces
that class's in FILE (default: the checkout's measurements/attention.json); other classes stay as they were. Each
kernel's build identity is computed from the Runtime source the pod ran (the wheel's stamped commit), by the checkout's
`scripts/attention_table.py identities`. Then `scripts/attention_table.py generate` makes the table."""

import argparse
import json
import statistics
import subprocess
import sys
import tarfile
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

SCHEMA = "cozy.attention-measurements/1"
type Json = Any  # type: ignore[explicit-any]  # a run record and its result document, read by key


def _record(path: Path) -> Json:
    return json.loads(path.read_text())


def _detail(record: Json) -> Json:
    return json.loads((record.get("result") or {}).get("detail") or "{}")


def _completed(gate: Path, pattern: str) -> list[tuple[Path, Json]]:
    found = []
    for path in sorted(gate.glob(pattern)):
        if path.name.endswith(("-weights.json", "-weights-off.json")):
            continue
        record = _record(path)
        if record.get("status") == "completed":
            found.append((path, record))
    return found


def _driver(record: Json) -> list[Json]:
    rows: list[Json] = []

    def walk(o: Json) -> None:
        if isinstance(o, dict):
            if isinstance(o.get("driver"), list):
                rows.extend(r for r in o["driver"] if isinstance(r, dict))
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(record.get("measurements"))
    return rows


def _identities(runtime: Path, commit: str) -> dict[str, str]:
    """Each kernel's identity in the Runtime source at COMMIT, by this checkout's script."""
    with tempfile.TemporaryDirectory(prefix="attention-src-") as tmp:
        archive = subprocess.run(["git", "-C", str(runtime), "archive", "--format=tar", commit, "src", "kernels"],
                                 capture_output=True, check=False)  # fmt: skip
        if archive.returncode:
            subprocess.run(["git", "-C", str(runtime), "fetch", "-q", "origin", commit], check=True)
            archive = subprocess.run(["git", "-C", str(runtime), "archive", "--format=tar", commit, "src", "kernels"],
                                     capture_output=True, check=True)  # fmt: skip
        tar = Path(tmp) / "src.tar"
        tar.write_bytes(archive.stdout)
        with tarfile.open(tar) as t:
            t.extractall(tmp, filter="data")
        done = subprocess.run(
            ["uv", "run", "--project", str(runtime), "--frozen", "--no-dev", "python",
             str(runtime / "scripts" / "attention_table.py"), "identities", "--src", str(Path(tmp) / "src")],
            capture_output=True, text=True, check=True,
        )  # fmt: skip
    found: dict[str, str] = json.loads(done.stdout)
    return found


def _merge_points(sets: list[Json]) -> list[Json]:
    """One row per point. Sets that ran in separate processes are put on one scale by SDPA, which runs in every set:
    a kernel's times from a set are scaled by the first set's SDPA median over that set's own."""
    rows: dict[str, Json] = {}
    anchor: dict[str, float] = {}
    for detail in sets:
        for p in detail["points"]:
            row = rows.setdefault(p["id"], {**p, "times": {}, "refused": {}, "failed": {}})
            sdpa = p["times"].get("sdpa", {}).get("ms", [0.0])[0]
            scale = anchor[p["id"]] / sdpa if p["id"] in anchor and sdpa > 0 else 1.0
            anchor.setdefault(p["id"], sdpa)
            for k, t in p["times"].items():
                if k not in row["times"]:
                    row["times"][k] = {**t, "ms": [round(x * scale, 5) for x in t["ms"]]}
            for key in ("refused", "failed"):
                for k, why in p.get(key, {}).items():
                    row[key].setdefault(k, why)
    return [rows[i] for i in sorted(rows)]


def collect(gate: Path, runtime: Path) -> tuple[str, Json]:
    inventory_path = gate / "attention-inventory.json"
    inventory = _detail(_record(inventory_path))
    if not inventory:
        raise SystemExit(f"{gate}: no completed attention-inventory")
    speed = _completed(gate, "attention-speed-*.json")
    quality = _completed(gate, "attention-quality-*.json")
    if not speed:
        raise SystemExit(f"{gate}: no completed attention-speed run")
    details = [_detail(r) for _, r in speed]
    for d in details:
        if d["inventory"]["commit"] != inventory["commit"]:
            raise SystemExit(f"{gate}: runs on two Runtime builds ({d['inventory']['commit']}, {inventory['commit']})")
    if inventory["commit"] in ("", "unknown"):
        raise SystemExit(f"{gate}: the Runtime the pod ran names no commit (an unstamped wheel): its kernels' builds are unknown")
    absent: dict[str, str] = {}
    for d in details:
        absent.update(d.get("absent", {}))
    measured = {k for d in details for p in d["points"] for k in (*p["times"], *p.get("refused", {}), *p.get("failed", {}))}
    absent = {k: why for k, why in absent.items() if k not in measured}
    driver = [row for _, r in speed for row in _driver(r)]
    records = [r for _, r in (*speed, *quality)] + [_record(inventory_path)]
    settled = max(int(r.get("settled_at_ms") or 0) for r in records)
    by_kernel: dict[str, Json] = {}
    for _, r in quality:
        d = _detail(r)
        by_kernel[d["kernel"]] = {k: d[k] for k in ("level", "reference", "corpus", "errors", "skipped")}
    record = {
        "card": inventory["card"], "sm": inventory["sm"], "row": inventory["row"],
        "half_accumulate": inventory["half_accumulate"], "runtime": inventory["runtime"], "commit": inventory["commit"],
        "torch": inventory["torch"], "cuda": inventory["cuda"], "driver": inventory["driver"],
        "measured": datetime.fromtimestamp(settled / 1000, UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "runs": sorted(int(r["number"]) for r in records),
        "corpus": sorted({q["corpus"] for q in by_kernel.values()} or {"none"})[0],
        "kernels": {k: v for k, v in _identities(runtime, inventory["commit"]).items() if k in inventory["kernels"]},
        "absent": absent,
        "pod": {
            "sm_mhz_mean": round(statistics.mean(r["sm_mhz_mean"] for r in driver if r.get("sm_mhz_mean")), 1) if driver else None,
            "sm_mhz_low": min((r["sm_mhz_low"] for r in driver if r.get("sm_mhz_low")), default=None),
            "throttled_share": max((r.get("throttled_share") or 0.0 for r in driver), default=None),
            "throttles": sorted({t for r in driver for t in r.get("throttles") or []}),
            "temperature_max_c": max((r.get("temperature_max_c") or 0 for r in driver), default=None),
            "power_max_w": max((r.get("power_max_w") or 0 for r in driver), default=None),
        },
        "points": _merge_points(details),
        "quality": by_kernel,
    }  # fmt: skip
    return str(inventory["klass"]), record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runtime", type=Path, required=True, help="a cozy-runtime checkout")
    parser.add_argument("--into", type=Path, help="default: <runtime>/measurements/attention.json")
    parser.add_argument("gates", type=Path, nargs="+")
    args = parser.parse_args()
    into: Path = args.into or args.runtime / "measurements" / "attention.json"
    data: Json = json.loads(into.read_text()) if into.exists() else {"schema": SCHEMA, "classes": {}}
    if data.get("schema") != SCHEMA:
        raise SystemExit(f"{into}: schema {data.get('schema')!r}, not {SCHEMA}")
    for gate in args.gates:
        klass, record = collect(gate, args.runtime.resolve())
        if klass in data["classes"]:
            print(f"{klass}: replaces runs {data['classes'][klass]['runs']}", file=sys.stderr)
        data["classes"][klass] = record
        throttles = record["pod"]["throttles"]
        print(f"{klass}: {record['card']}, Runtime {record['runtime']} ({record['commit'][:9]}), {len(record['points'])} points, "
              f"{len(record['quality'])} kernels judged{', throttled ' + ','.join(throttles) if throttles else ''}", file=sys.stderr)  # fmt: skip
    data["classes"] = dict(sorted(data["classes"].items()))
    into.parent.mkdir(parents=True, exist_ok=True)
    into.write_text(json.dumps(data, indent=1, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
