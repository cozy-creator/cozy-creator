"""cold-load's table (sessions/cold-load.sh): each cell's time until inference can start, against the machine's bound.

    python3 -I table.py <session dir>

It reads <dir>/cold-load.tsv (label, arm, model, condition, loop stage, record file, census file and, optionally, the warm
step: step2 or hot; one cell a line) and each
record. Prints a JSON row per cell, then the cells and the medians by arm, model and condition as Markdown tables.

- Ready: when the loop's (`loop`: H3's `denoise`) first step could have begun with everything in place: its end less a warm
  step's compute, from the run's acceptance on the machine, and never before the stages ahead of the loop end. The warm
  step is the run's own step 2 (`warm` step2: by then every block was in place; a model whose blocks stream during steps,
  H3 on a 24 GB card, streams in it too, so streaming stays in step time), or the arm's `hot` run's step 1 (`warm` hot).
  H3's hot step 1 is no reference: a fresh executor's runs it dense (`TURBO_DENSE_STEPS`), 48 s on a 4090, and the same
  executor's next request ran it in 25 s.
- Its parts, on the record's own clock: executor start (to construction), construction, models built on demand inside the
  request (`on_demand`: Qwen's prompt enhancer), each stage before the loop (its
  `<stage> / loading <slot>` loads apart), and from there to ready (the loop's model load, its LoRA bake, graph capture).
  `gap` is what none of them covers. The LoRA bake's host and device seconds are varena's (`moved.bake`). `load` is ready
  less the stages' own compute ahead of the loop (H3's text encode, Qwen's prompt enhancer writing its prompt).
- Bytes, from varena's fill trace: every block of each set a fill used by step 1's end (a speculative fill has the time of
  the next timed one), once. `gpu` is that, less what the loop filled again after step 1 (the paged part, in RAM by step 1).
- Bound: `disk` = bytes ÷ the record's disk GB/s, `pcie` = gpu ÷ its pinned H2D GB/s, both varena's `ceiling`; cold and disk
  take the larger, ram takes pcie. `x` = ready ÷ bound.
"""

import json
import os
import statistics
import sys
from typing import TypeAlias

Obj: TypeAlias = dict[str, object]


def obj(x: object) -> Obj:
    return x if isinstance(x, dict) else {}


def arr(x: object) -> list[object]:
    return x if isinstance(x, list) else []


def num(x: object) -> float:
    return float(x) if isinstance(x, (int, float)) and not isinstance(x, bool) else 0.0


def load(path: str) -> Obj:
    try:
        return obj(json.load(open(path)))
    except (OSError, ValueError):
        return {}


def measurements(rec: Obj) -> list[Obj]:
    """The run's own measurements, then each call's (H3's are its call's)."""
    out = [obj(rec.get("measurements"))] + [obj(obj(c).get("measurements")) for c in arr(rec.get("calls"))]
    return [m for m in out if m]


def track(rec: Obj, loop: str) -> tuple[Obj, Obj]:
    """The measurement with the loop's steps, and the steps: `loop` by name, else the longest that decodes nothing."""
    best: tuple[Obj, Obj] = ({}, {})
    for m in measurements(rec):
        steps = obj(obj(m.get("attribution")).get("steps"))
        if loop and loop in steps:
            return m, obj(steps[loop])
        for k, v in steps.items():
            if not loop and not k.startswith(("decod", "encod")) and num(obj(v).get("count")) > num(best[1].get("count")):
                best = (m, obj(v))
    return best


def first_step(steps: Obj) -> tuple[float, float]:
    """Step 1's end (Unix ms) and its ms; a 0 ms first entry is a warm-up mark."""
    series = [s for s in arr(steps.get("series")) if isinstance(s, list) and len(s) == 2]
    if series and num(series[0][1]) <= 0:
        series = series[1:]
    return (num(series[0][0]), num(series[0][1])) if series else (0.0, 0.0)


def varena(m: Obj) -> list[Obj]:
    return [obj(v) for v in arr(obj(obj(m.get("memory")).get("stats")).get("varena"))]


def timed(trace: list[Obj]) -> list[tuple[float, Obj]]:
    """Each fill with its time: a speculative one has none, so it takes the next timed fill's."""
    out: list[tuple[float, Obj]] = []
    nxt = float("inf")
    for e in sorted(trace, key=lambda e: num(e.get("sequence")), reverse=True):
        if "at_unix_ms" in e:
            nxt = num(e.get("at_unix_ms"))
        out.append((nxt, e))
    return out[::-1]


def blocks(fills: list[tuple[float, Obj]]) -> dict[str, dict[int, float]]:
    out: dict[str, dict[int, float]] = {}
    for _, e in fills:
        s = out.setdefault(str(e.get("set")), {})
        b = int(num(e.get("block")))
        s[b] = max(s.get(b, 0.0), num(e.get("bytes")))
    return out


def short(name: str) -> str:
    return name.rsplit("/", 1)[-1]


def row(label: str, arm: str, model: str, condition: str, loop: str, warm: str, rec: Obj, hot: Obj, census: Obj) -> Obj:
    m, steps = track(rec, loop)
    t0 = num(rec.get("accepted_at_ms")) or num(rec.get("started_at_ms"))
    series = [s for s in arr(steps.get("series")) if isinstance(s, list) and len(s) == 2]
    if series and num(series[0][1]) <= 0:
        series = series[1:]
    flaws: list[str] = []
    if not series:
        return {"label": label, "arm": arm, "model": model, "condition": condition, "status": rec.get("status"),
                "flaws": ["no loop steps in the record"]}
    end1, ms1 = num(series[0][0]), num(series[0][1])
    _, hot1 = first_step(track(hot, loop)[1]) if hot else (0.0, 0.0)
    warm_ms = hot1 if warm == "hot" or len(series) < 2 else num(series[1][1])
    if not warm_ms:
        flaws.append("no warm step: ready is step 1's end")
    execution = obj(m.get("execution"))
    construction = obj(execution.get("construction"))
    prepared, prepare = num(construction.get("prepared_unix_ms")), num(construction.get("prepare_ms"))
    began = prepared - prepare if prepared else t0
    executor = obj(execution.get("executor"))
    if not executor or not num(executor.get("started_unix_ms")):
        flaws.append("no executor start: not a fresh executor")
    stages = {k: obj(v) for k, v in obj(obj(m.get("attribution")).get("stages")).items()}
    top = {k: v for k, v in stages.items() if " / " not in k and not k.startswith("host · ")}
    before = sorted((k for k, v in top.items() if k != loop and not k.startswith(("decod", "saving"))
                     and num(v.get("started_unix_ms")) < end1), key=lambda k: num(top[k].get("started_unix_ms")))
    parts: list[Obj] = []
    pre_end = prepared or t0
    pre_load = pre_compute = 0.0
    for k in before:
        carved = {n.split(" / ", 1)[1]: v for n, v in stages.items() if n.startswith(k + " / ")}
        loads = {n[len("loading "):]: v for n, v in carved.items() if n.startswith("loading ")}
        own = num(top[k].get("total_ms"))
        pre_load += sum(num(v.get("total_ms")) for v in loads.values())
        pre_compute += own
        pre_end = max([pre_end, num(top[k].get("ended_unix_ms"))] + [num(v.get("ended_unix_ms")) for v in carved.values()])
        parts.append({"stage": k, "s": round(own / 1000, 2),
                      "carved": {n: round(num(v.get("total_ms")) / 1000, 2) for n, v in carved.items()}})
    ready_at = max(pre_end, end1 - warm_ms)
    # Models built on demand, inside the request (Qwen's prompt enhancer, a LoRA), before ready.
    built = {k: obj(v) for k, v in obj(execution.get("on_demand")).items()}
    on_demand = sum(num(v.get("prepare_ms")) for v in built.values() if 0 < num(v.get("prepared_unix_ms")) <= ready_at)
    loop_loads = {n.split(" / loading ", 1)[1]: round(num(v.get("total_ms")) / 1000, 2)
                  for n, v in stages.items() if n.startswith(loop + " / loading ")}
    # First output: the first decoded step or decode stage.
    outs = [first_step(obj(v))[0] for k, v in obj(obj(m.get("attribution")).get("steps")).items() if k.startswith("decod")]
    outs += [num(v.get("ended_unix_ms")) for k, v in top.items() if k.startswith(("decod", "saving"))]
    outs = [o for o in outs if o]
    first_out = min(outs) if outs else t0 + num(rec.get("execution_ms"))
    # Bytes in place by step 1.
    vs = varena(m)
    ceiling = obj(vs[0].get("ceiling")) if vs else {}
    disk_gbps, h2d_gbps = num(ceiling.get("disk_gbps")), num(ceiling.get("h2d_gbps"))
    fills = [f for v in vs for f in timed([obj(e) for e in arr(obj(v.get("stats")).get("trace"))])]
    used = {str(e.get("set")) for t, e in fills if t <= end1 and not e.get("speculative")}
    sizes = {s: sum(b.values()) for s, b in blocks([f for f in fills if str(f[1].get("set")) in used]).items()}
    # The paged part: what the loop fills again after its first step; a block an earlier stage evicted comes back in step 1.
    paged = {s: sum(b.values()) for s, b in blocks([f for f in fills if end1 < f[0] <= num(steps.get("ended_unix_ms"))
                                                    and str(f[1].get("set")) in used]).items()}
    need = sum(sizes.values())
    gpu = sum(max(0.0, v - paged.get(s, 0.0)) for s, v in sizes.items())
    by_origin: dict[str, float] = {}
    for t, e in fills:
        if t <= end1:
            by_origin[str(e.get("origin"))] = by_origin.get(str(e.get("origin")), 0.0) + num(e.get("bytes"))
    disk_s = need / (disk_gbps * 1e9) if disk_gbps else 0.0
    pcie_s = gpu / (h2d_gbps * 1e9) if h2d_gbps else 0.0
    bound = pcie_s if condition == "ram" else max(disk_s, pcie_s)
    if not disk_gbps or not h2d_gbps:
        flaws.append("no ceiling in the record")
    bake = {"blocks": 0.0, "host_ns": 0.0, "device_ns": 0.0}
    persisted = 0.0
    for mm in measurements(rec):
        for v in varena(mm):
            st = obj(v.get("stats"))
            b = obj(obj(st.get("moved")).get("bake"))
            for k in bake:
                bake[k] += num(b.get(k))
            persisted += num(obj(st.get("baked_pages")).get("persisted_bytes"))
    ready = (ready_at - t0) / 1000
    pieces = (began - t0 + prepare + on_demand + pre_load + pre_compute + ready_at - pre_end) / 1000
    calls = [obj(c) for c in arr(rec.get("calls"))]
    return {
        "label": label, "arm": arm, "model": model, "condition": condition, "run": rec.get("name"),
        "status": rec.get("status"), "machine": rec.get("machine"),
        "ready_s": round(ready, 2), "bound_s": round(bound, 2), "x": round(ready / bound, 2) if bound else None,
        "load_s": round(ready - pre_compute / 1000, 2),
        "x_load": round((ready - pre_compute / 1000) / bound, 2) if bound else None,
        "need_gb": round(need / 1e9, 2), "gpu_gb": round(gpu / 1e9, 2), "disk_s": round(disk_s, 2), "pcie_s": round(pcie_s, 2),
        "disk_gbps": round(disk_gbps, 2), "h2d_gbps": round(h2d_gbps, 1),
        "wait_s": round((t0 - num(rec.get("accepted_at_ms"))) / 1000, 2) if rec.get("accepted_at_ms") else None,
        "executor_s": round((began - t0) / 1000, 2), "construct_s": round(prepare / 1000, 2),
        "on_demand_s": round(on_demand / 1000, 2), "pre_load_s": round(pre_load / 1000, 2),
        "pre_compute_s": round(pre_compute / 1000, 2),
        "to_step1_s": round((ready_at - pre_end) / 1000, 2), "gap_s": round(ready - pieces, 2),
        "bake_blocks": int(bake["blocks"]), "bake_host_s": round(bake["host_ns"] / 1e9, 2),
        "bake_device_s": round(bake["device_ns"] / 1e9, 2), "persisted_gb": round(persisted / 1e9, 2),
        "step1_s": round(ms1 / 1000, 2), "warm_step_s": round(warm_ms / 1000, 2),
        "first_output_s": round((first_out - t0) / 1000, 2), "total_s": round(num(rec.get("execution_ms")) / 1000, 2),
        "loop_loads_s": loop_loads, "stages": parts,
        "sets_gb": {short(s): round(v / 1e9, 2) for s, v in sizes.items()},
        "paged_gb": {short(s): round(v / 1e9, 2) for s, v in paged.items()},
        "filled_by_step1_gb": {k: round(v / 1e9, 2) for k, v in by_origin.items()},
        "store_resident_gb": census.get("store_resident_gb"), "store_gb": census.get("store_gb"),
        "derived_records": census.get("derived_records"), "drop": census.get("drop_caches"),
        "on_demand": {k: round(num(v.get("prepare_ms")) / 1000, 2) for k, v in built.items()},
        "calls": [{"function": c.get("function"), "memoized": c.get("memoized")} for c in calls],
        "flaws": flaws,
    }


COLS = ["label", "run", "ready_s", "bound_s", "x", "load_s", "x_load", "need_gb", "gpu_gb", "disk_s", "pcie_s", "executor_s", "construct_s",
        "on_demand_s", "pre_load_s", "pre_compute_s", "to_step1_s", "gap_s", "bake_host_s", "bake_device_s", "step1_s", "warm_step_s",
        "first_output_s", "total_s", "store_resident_gb"]
MEDIAN = ["ready_s", "bound_s", "x", "load_s", "x_load", "need_gb", "gpu_gb", "disk_s", "pcie_s", "executor_s", "construct_s",
          "on_demand_s", "pre_load_s", "pre_compute_s", "to_step1_s", "gap_s", "bake_host_s", "bake_device_s", "step1_s", "first_output_s", "total_s"]


def table(rows: list[Obj], cols: list[str]) -> str:
    lines = ["| " + " | ".join(cols) + " |", "|" + "---|" * len(cols)]
    for r in rows:
        lines.append("| " + " | ".join("" if r.get(c) is None else str(r.get(c)) for c in cols) + " |")
    return "\n".join(lines)


def main() -> None:
    d = sys.argv[1]
    cells = [line.rstrip("\n").split("\t") for line in open(os.path.join(d, "cold-load.tsv")) if line.strip()]
    recs = {c[0]: load(os.path.join(d, c[5])) for c in cells}
    hots = {(c[1], c[2]): recs[c[0]] for c in cells if c[3] == "hot"}
    strays = load(os.path.join(d, "strays.json"))
    rows = []
    for label, arm, model, condition, loop, record, census, *rest in cells:
        if condition in ("hot", "warmup"):
            continue
        rows.append(row(label, arm, model, condition, loop, rest[0] if rest else "step2", recs[label], hots.get((arm, model), {}),
                        load(os.path.join(d, census)) if census else {}))
        if strays.get(label):
            rows[-1].setdefault("flaws", []).append(f"stray runs on the pod: {', '.join(map(str, arr(strays[label])))}")
    for r in rows:
        print(json.dumps(r))
    print()
    print(table(rows, COLS))
    groups: dict[tuple[str, str, str], list[Obj]] = {}
    for r in rows:
        if r.get("status") == "completed" and not any(w in f for f in map(str, arr(r.get("flaws"))) for w in ("fresh", "stray")):
            groups.setdefault((str(r["arm"]), str(r["model"]), str(r["condition"])), []).append(r)
    med = []
    for (arm, model, condition), rs in groups.items():
        m: Obj = {"arm": arm, "model": model, "condition": condition, "n": len(rs)}
        for k in MEDIAN:
            vals = [num(r.get(k)) for r in rs if r.get(k) is not None]
            m[k] = round(statistics.median(vals), 2) if vals else None
        med.append(m)
    print()
    print(table(med, ["arm", "model", "condition", "n"] + MEDIAN))


main()
