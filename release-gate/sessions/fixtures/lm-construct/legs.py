"""lm-construct's table (sessions/lm-construct.sh): each run record's start-up as parts, seconds, against the ceilings.

    python3 -I legs.py <ceiling.json> <bar.json> <label>=<run record> ...

A label is <arm>-<case> (case: first, cold, warm, hot, sdxl-new, sdxl-warm). Each row: its wait before the executor (download,
install, queue), executor start, construction (prepare) and its fill legs, the time from construction to the answer's
first weight read, the load of the weights it read (bytes and GB/s against the store's ceiling), and the rest. The
bar (`bar.json`) is checked on every arm's Qwen3.6 rows and fails the session for a judged arm: a warm construction
under `prepare_s`, and a cold run's execution under `x` times the weights it read at the disk's ceiling plus `fixed_s`.
"""

import json
import sys
from typing import TypeAlias

Obj: TypeAlias = dict[str, object]


def obj(x: object) -> Obj:
    return x if isinstance(x, dict) else {}


def num(x: object) -> float:
    return float(x) if isinstance(x, (int, float)) and not isinstance(x, bool) else 0.0


def first(x: object) -> Obj:
    return obj(x[0]) if isinstance(x, list) and x else {}


def stage(record: Obj, name: str) -> float:
    stages = obj(obj(obj(record.get("measurements")).get("attribution")).get("stages"))
    return num(obj(stages.get(name)).get("total_ms"))


def row(label: str, record: Obj, ceiling: Obj) -> Obj:
    m = obj(record.get("measurements"))
    construction = obj(obj(m.get("execution")).get("construction"))
    legs = {k: num(v) for k, v in obj(construction.get("legs_ms")).items()}
    executor = obj(obj(m.get("execution")).get("executor"))
    memory = first(first(obj(record.get("report")).get("measured")).get("memory"))
    store = obj(memory.get("store"))
    accepted, started = num(record.get("accepted_at_ms")), num(record.get("started_at_ms"))
    execution = num(record.get("execution_ms"))
    prepare = num(construction.get("prepare_ms"))
    load = stage(record, "answering / loading model")
    answering = stage(record, "answering")
    store_gbps = num(store.get("gbps"))
    disk = num(ceiling.get("disk_gbps"))
    h2d = num(ceiling.get("h2d_gbps"))
    text = obj(record.get("result")).get("text")
    return {
        "label": label,
        "run": record.get("name"),
        "status": record.get("status"),
        "wait_s": round((started - accepted) / 1000, 1) if started and accepted else None,
        "executor_s": round(num(executor.get("ms")) / 1000, 1),
        "prepare_s": round(prepare / 1000, 1),
        "factory_s": round(legs.get("construct.factory", 0) / 1000, 1),
        "fill_s": round(legs.get("construct.fill", 0) / 1000, 1),
        "leaves_s": round(legs.get("register.leaves", 0) / 1000, 1),
        "routes_s": round(sum(v for k, v in legs.items() if k.startswith("register.") and k.endswith(".routes")) / 1000, 1),
        "register_s": round(sum(v for k, v in legs.items() if k.startswith("register.") and k.endswith(".load")) / 1000, 1),
        "answer_s": round(answering / 1000, 1),
        "load_s": round(load / 1000, 1),
        "execution_s": round(execution / 1000, 1),
        "store_gb": round(num(store.get("bytes")) / 1e9, 2),
        "store_gbps": round(store_gbps, 2) if store_gbps else None,
        "disk_ceiling_gbps": round(disk, 2) if disk else None,
        "h2d_ceiling_gbps": round(h2d, 1) if h2d else None,
        "peak_gb": round(num(obj(m.get("memory")).get("peak_reserved_bytes")) / 1e9, 1),
        "text": (text if isinstance(text, str) else "")[:160],
    }


ceiling = json.load(open(sys.argv[1]))
bars = json.load(open(sys.argv[2]))
rows: list[Obj] = []
failed = False
for pair in sys.argv[3:]:
    label, path = pair.split("=", 1)
    try:
        record = json.load(open(path))
    except (OSError, ValueError) as exc:
        rows.append({"label": label, "status": f"unrecorded: {exc}"})
        continue
    rows.append(row(label, record, ceiling))
for each in rows:
    print(json.dumps(each))
cols = ["label", "run", "status", "wait_s", "executor_s", "prepare_s", "factory_s", "fill_s", "leaves_s", "routes_s",
        "register_s", "answer_s", "load_s", "execution_s", "store_gb", "store_gbps", "disk_ceiling_gbps", "peak_gb"]
print("| " + " | ".join(cols) + " |")
print("|" + "---|" * len(cols))
for each in rows:
    print("| " + " | ".join("" if each.get(c) is None else str(each.get(c)) for c in cols) + " |")
disk = ceiling.get("disk_gbps") or 0
# Every arm's verdict is said; only a judged arm's fails the session (an unjudged one is the red beside it).
for arm, bar in bars.items():
    judged = bool(bar.get("judged"))
    for case in ("warm", "cold"):
        r = next((r for r in rows if r["label"] == f"{arm}-{case}"), None)
        if r is None or r.get("status") != "completed":
            print(f"FAIL {arm}-{case}: no completed Qwen3.6 run{'' if judged else ' (not judged)'}")
            failed |= judged
            continue
        if case == "warm":
            ok, said = num(r.get("prepare_s")) <= bar["prepare_s"], f"construction {r['prepare_s']} s, bar {bar['prepare_s']} s"
        else:
            limit = round(bar["x"] * num(r.get("store_gb")) / disk + bar["fixed_s"], 1) if disk else None
            ok = limit is not None and num(r.get("execution_s")) <= limit
            said = (f"execution {r['execution_s']} s, bar {limit} s ({bar['x']} x {r['store_gb']} GB at the disk's "
                    f"{disk:.2f} GB/s + {bar['fixed_s']} s)")
        print(f"{'PASS' if ok else 'FAIL'} {arm}-{case}: {said}{'' if judged else ' (not judged)'}")
        failed |= judged and not ok
sys.exit(1 if failed else 0)
