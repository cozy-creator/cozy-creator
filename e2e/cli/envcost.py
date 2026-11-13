"""What package environments cost on disk (cl-984), for e2e/cli/envcost.sh.

`measure ROOT...`: each root's allocated bytes by area (its first path part named one: generations, editable, torch-*, python,
uv-cache, toolchain, else other), a hard-linked file counted once and under the first area it is in; the roots together, each
file once; the disk's used bytes. `row`: one install's record from its events and the measures before and after it. `table`:
the rows as Markdown.
"""

import json
import os
import sys
from pathlib import Path

AREAS = ["generations", "editable", "torch", "python", "toolchain", "uv-cache", "objects", "other"]
ENVIRONMENTS = {"generations", "editable"}


def area(parts: tuple[str, ...]) -> str:
    for part in parts:
        if part in ("generations", "editable", "python", "uv-cache", "toolchain", "objects"):
            return part
        if part.startswith("torch-") or part == ".torch.partial":
            return "torch"
    return "other"


def walk(root: Path) -> dict[tuple[int, int], tuple[int, str]]:
    """Each file's inode: its allocated bytes and the first area it is in."""
    found: dict[tuple[int, int], tuple[int, str]] = {}
    if not root.exists():
        return found
    for top, dirs, files in os.walk(root, followlinks=False):
        rel = Path(top).relative_to(root).parts
        for name in files + [d for d in dirs if os.path.islink(os.path.join(top, d))]:
            try:
                st = os.lstat(os.path.join(top, name))
            except FileNotFoundError:
                continue
            key = (st.st_dev, st.st_ino)
            here = area(rel + (name,))
            if key not in found or AREAS.index(here) < AREAS.index(found[key][1]):
                found[key] = (st.st_blocks * 512, here)
    return found


def summed(files: dict[tuple[int, int], tuple[int, str]]) -> dict[str, int]:
    out = {name: 0 for name in AREAS}
    for size, name in files.values():
        out[name] += size
    out["environments"] = out["generations"] + out["editable"]
    out["total"] = sum(out[name] for name in AREAS)
    return out


def measure(roots: list[str]) -> dict:
    every: dict[tuple[int, int], tuple[int, str]] = {}
    out: dict = {"roots": {}}
    for root in roots:
        files = walk(Path(root))
        out["roots"][root] = summed(files)
        for key, value in files.items():
            every.setdefault(key, value)
    out["together"] = summed(every)
    fs = os.statvfs(roots[0] if Path(roots[0]).exists() else "/")
    out["disk"] = {"used": (fs.f_blocks - fs.f_bfree) * fs.f_frsize, "available": fs.f_bavail * fs.f_frsize}
    return out


def events(path: Path) -> dict:
    stages: list[tuple[str, int]] = []
    fetched: dict[str, int] = {}
    runs: dict[int, dict] = {}
    error = None
    for line in path.read_text(errors="replace").splitlines():
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        progress = event.get("progress") or {}
        if event.get("type") == "run.progress" and progress.get("stage") and progress.get("at_ms"):
            if not stages or stages[-1][0] != progress["stage"]:
                stages.append((progress["stage"], progress["at_ms"]))
            if "downloading" in progress["stage"] and "the result" not in progress["stage"]:
                fetched[progress["stage"]] = max(fetched.get(progress["stage"], 0), progress.get("bytes_done") or 0)
        run = event.get("run") if isinstance(event.get("run"), dict) else None
        if run and run.get("number"):
            runs[run["number"]] = run
        if isinstance(event.get("error"), dict):
            error = event["error"]
    timed = []
    for i, (stage, at) in enumerate(stages):
        end = stages[i + 1][1] if i + 1 < len(stages) else None
        timed.append({"stage": stage, "s": None if end is None else round((end - at) / 1000, 1)})
    last = runs[max(runs)] if runs else {}
    span = None
    if last.get("settled_at_ms") and last.get("created_at_ms"):
        span = round((last["settled_at_ms"] - last["created_at_ms"]) / 1000, 1)
    reason = (last.get("reason") or {}).get("message")
    between = lambda a, b: round((last[b] - last[a]) / 1000, 1) if last.get(a) and last.get(b) else None
    return {"runs": sorted(runs), "status": last.get("status"), "run_s": span, "stages": timed,
            "reason": reason, "error": error, "downloaded": sum(fetched.values()),
            "preparing_s": between("accepted_at_ms", "started_at_ms"), "executing_s": between("started_at_ms", "settled_at_ms")}


def row(args: list[str]) -> dict:
    label, project, before, after, ndjson, wall, code = args[:7]
    extra = json.loads(Path(args[7]).read_text()) if len(args) > 7 else {}
    b, a = json.loads(Path(before).read_text()), json.loads(Path(after).read_text())
    deltas = {root: {name: a["roots"][root][name] - b["roots"][root][name] for name in a["roots"][root]} for root in a["roots"]}
    together = {name: a["together"][name] - b["together"][name] for name in a["together"]}
    return {"label": label, "project": project, "wall_s": float(wall), "exit": int(code), **events(Path(ndjson)),
            "deltas": deltas, "together_delta": together, "disk_used_delta": a["disk"]["used"] - b["disk"]["used"],
            "after": a, **extra}


def gb(n: int) -> str:
    return f"{n / 1e9:.2f}"


def table(path: str) -> str:
    every = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    rows = [r for r in every if r.get("kind") != "run"]
    out = ["| step | exit | wall s | run s | envs Δ GB | torch base Δ GB | uv-cache Δ GB | python Δ GB | its root Δ GB | all roots Δ GB | disk used Δ GB |",
           "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in rows:
        d = r["deltas"][r["project"]]
        out.append(f"| {r['label']} | {r['exit']} | {r['wall_s']:.1f} | {r['run_s'] if r['run_s'] is not None else '-'} | {gb(d['environments'])} | "
                   f"{gb(d['torch'])} | {gb(d['uv-cache'])} | {gb(d['python'])} | {gb(d['total'])} | {gb(r['together_delta']['total'])} | {gb(r['disk_used_delta'])} |")
    if every:
        last = every[-1]["after"]
        out += ["", "| root | envs GB | torch base GB | uv-cache GB | python GB | other GB | total GB |", "|---|---|---|---|---|---|---|"]
        for root, s in list(last["roots"].items()) + [("all roots, each file once", last["together"])]:
            out.append(f"| {root} | {gb(s['environments'])} | {gb(s['torch'])} | {gb(s['uv-cache'])} | {gb(s['python'])} | {gb(s['other'])} | {gb(s['total'])} |")
        out.append(f"\ndisk: {gb(last['disk']['used'])} GB used, {gb(last['disk']['available'])} GB available")
    runs = [r for r in every if r.get("kind") == "run"]
    if runs:
        out += ["", "| run | exit | wall s | preparing s | executing s | downloaded MB | loopback rx MB | its objects Δ MB | ~/.cozy Δ MB | TensorFS home Δ MB | all roots Δ MB |",
                "|---|---|---|---|---|---|---|---|---|---|---|"]
        mb = lambda n: f"{n / 1e6:.1f}"
        for r in runs:
            d, home, tfs = r["deltas"][r["project"]], r["deltas"].get(r["home"], {}), r["deltas"].get(r["tensorfs"], {})
            out.append(f"| {r['label']} | {r['exit']} | {r['wall_s']:.1f} | {r['preparing_s']} | {r['executing_s']} | {mb(r['downloaded'])} | "
                       f"{mb(r['loopback'])} | {mb(d['objects'])} | {mb(home.get('total', 0))} | {mb(tfs.get('total', 0))} | {mb(r['together_delta']['total'])} |")
    for r in every:
        timed = "; ".join(f"{s['stage']} {s['s']}s" for s in r["stages"] if s["s"] is not None and (s["s"] >= 1 or r.get("kind") == "run"))
        out.append(f"\n{r['label']}: runs {r['runs']} {r['status']}{' — ' + r['reason'] if r['reason'] else ''}{' — ' + json.dumps(r['error']) if r['error'] else ''}")
        if timed:
            out.append(f"  stages ≥1 s: {timed}")
    return "\n".join(out)


if __name__ == "__main__":
    command, rest = sys.argv[1], sys.argv[2:]
    if command == "measure":
        print(json.dumps(measure(rest)))
    elif command == "row":
        print(json.dumps(row(rest)))
    elif command == "table":
        print(table(rest[0]))
    else:
        sys.exit(f"envcost.py: no command {command}")
