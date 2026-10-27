"""analyze.py <session OUT>: lowvram.sh's records as one row per request (rows.json) and a table per level (stdout).

Both engines: time (Cozy: the CLI's round trip and the machine's execution; ComfyUI: submit to saved image on the
pod), steady seconds per step, the engine's peak VRAM (NVML used minus the level's idle baseline, so the holder and
the driver are not counted), and H2D GB from the one NVML sampler. Cozy also: TensorFS store bytes, reclaimed bytes,
cut sites below level 0, prefetch wait."""
import json
import statistics
import sys
from pathlib import Path

OUT = Path(sys.argv[1])


def load(p: Path):
    try:
        return json.loads(p.read_text())
    except (OSError, ValueError):
        return None


def marks() -> dict[str, dict]:
    files = sorted(OUT.glob("out/collect/**/*marks*")) or [OUT / "tool.log"]
    found = {}
    for f in files:
        for line in f.read_text().splitlines():
            try:
                m = json.loads(line)
            except ValueError:
                continue
            if isinstance(m, dict) and "label" in m and "h2d_bytes" in m:
                found[m["label"]] = m
    return found


def mean(xs):
    xs = [x for x in xs if isinstance(x, (int, float))]
    return round(statistics.mean(xs), 3) if xs else None


def cozy_row(label: str, rec: dict, wall: dict, mk: dict, prev: dict | None, base: dict | None) -> dict:
    m = rec.get("measurements") or {}
    stages = (m.get("attribution") or {}).get("stages") or {}
    steps = (m.get("attribution") or {}).get("steps") or {}
    series = []
    if steps:
        name = "denoise" if "denoise" in steps else max(steps, key=lambda k: (steps[k] or {}).get("count", 0))
        series = [x[1] / 1000 for x in (steps[name] or {}).get("series", [])]
    varena = [((v or {}).get("stats") or {}) for v in ((m.get("memory") or {}).get("stats") or {}).get("varena") or []]
    v = varena[0] if varena else {}
    phases = v.get("phases") or []
    sites = ((m.get("execution") or {}).get("cuts") or {}).get("sites") or []
    driver = (m.get("memory") or {}).get("driver") or [{}]
    return {
        "label": label, "engine": "cozy", "status": rec.get("status"), "run": rec.get("number"),
        "wall_s": round(wall.get("wall_ms", 0) / 1000, 2) if wall else None,
        "exec_s": round((rec.get("execution_ms") or 0) / 1000, 2),
        "hub_s": round(((rec.get("settled_at_ms") or 0) - (rec.get("created_at_ms") or 0)) / 1000, 2),
        "denoise_s": round((stages.get("denoise") or {}).get("total_ms", 0) / 1000, 2) if stages.get("denoise") else None,
        "step1_s": round(series[0], 3) if series else None,
        "steady_s": mean(series[1:]) if len(series) > 1 else None,
        "executor_s": round(((m.get("execution") or {}).get("executor") or {}).get("ms", 0) / 1000, 2),
        "prepare_s": round((((m.get("execution") or {}).get("construction") or {}).get("prepare_ms") or 0) / 1000, 2),
        "peak_engine_mib": (mk["peak_used_mib"] - base["used_mib"]) if mk and base else None,
        "h2d_gb": round((mk["h2d_bytes"] - prev["h2d_bytes"]) / 1e9, 2) if mk and prev else None,
        "record_peak_driver_gib": round((driver[0].get("peak_bytes") or 0) / 2**30, 2),
        "torch_peak_reserved_gib": round(((m.get("memory") or {}).get("peak_reserved_bytes") or 0) / 2**30, 2),
        "store_gb": round((((v.get("moved") or {}).get("store") or {}).get("bytes") or 0) / 1e9, 2),
        "reclaimed_gb": round(sum(p.get("native_reclaimed_bytes") or 0 for p in phases) / 1e9, 2),
        "torch_returned_gb": round(sum(p.get("torch_returned_bytes") or 0 for p in phases) / 1e9, 2),
        "prefetch_wait_s": round(((v.get("prefetch") or {}).get("wait_ns") or 0) / 1e9, 2),
        "cut_sites": len(sites), "cut_deep": sum(1 for s in sites if (s.get("level") or 0) > 0),
        "cut_retries": sum(s.get("retries") or 0 for s in sites),
        "software": (m.get("software") or {}),
    }


def comfy_rows(session: dict, base: dict | None) -> list[dict]:
    out = []
    for label, r in (session.get("per_request") or {}).items():
        steps = r.get("step_s") or []
        loads = sum(s[1] for s in r.get("segments") or [] if s[0] and ("Loader" in s[0]))
        out.append({"label": label, "engine": "comfy", "status": r.get("status"), "wall_s": r.get("total_s"),
                    "exec_s": r.get("total_s"), "server_start_s": session.get("server_start_s"),
                    "first_step_at_s": r.get("first_step_at_s"), "loader_s": round(loads, 2),
                    "steady_s": mean(steps[1:]) if len(steps) > 1 else None, "step1_s": steps[0] if steps else None,
                    "peak_engine_mib": (r["peak_used_mib"] - base["used_mib"]) if "peak_used_mib" in r and base else None,
                    "h2d_gb": r.get("h2d_gb"), "error": r.get("error") or None})
    return out


def main() -> None:
    mk = marks()
    walls = {}
    if (OUT / "walls.jsonl").exists():
        for line in (OUT / "walls.jsonl").read_text().splitlines():
            w = json.loads(line)
            walls[w["label"]] = w
    rows = []
    for f in sorted(OUT.glob("comfy-*.session.json")):
        s = load(f) or {}
        name = f.name.removesuffix(".session.json")
        rows += comfy_rows(s, mk.get(f"{name}-start"))
    for f in sorted((OUT / "rec").glob("*.json")):
        label = f.stem
        parts = label.split("-")
        if len(parts) < 4 or parts[-1] not in ("cold", "w1", "w2", "w3") or label.startswith(("comfy-", "mark-", "hold-", "evict-")):
            continue
        rec = load(f) or {}
        group = "-".join(parts[:-1])
        prev = mk.get(f"{group}-start") if parts[-1] == "cold" else mk.get(f"{group}-{ {'w1': 'cold', 'w2': 'w1', 'w3': 'w2'}[parts[-1]] }")
        rows.append(cozy_row(label, rec, walls.get(label), mk.get(label), prev, mk.get(f"{group}-start")))
    for r in rows:
        arm, *rest = r["label"].split("-")
        r["arm"], r["level"], r["model"], r["tier"] = arm, rest[0], rest[1], rest[2]
    (OUT / "rows.json").write_text(json.dumps(rows, indent=1))
    cols = ["arm", "tier", "status", "wall_s", "hub_s", "exec_s", "step1_s", "steady_s", "peak_engine_mib", "h2d_gb", "store_gb",
            "reclaimed_gb", "cut_deep", "run"]
    for level in dict.fromkeys(r["level"] for r in rows):
        for model in dict.fromkeys(r["model"] for r in rows if r["level"] == level):
            print(f"\n### {model} at {'the full card' if level == 'full' else level + ' MiB free'}\n")
            print("| " + " | ".join(cols) + " |")
            print("|" + "---|" * len(cols))
            for r in rows:
                if r["level"] == level and r["model"] == model:
                    print("| " + " | ".join("-" if r.get(c) is None else str(r.get(c)) for c in cols) + " |")


if __name__ == "__main__":
    main()
