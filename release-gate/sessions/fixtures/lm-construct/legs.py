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


def stage(record, name):
    return (record.get("measurements", {}).get("attribution", {}).get("stages", {}).get(name) or {}).get("total_ms", 0.0)


def row(label, record, ceiling):
    m = record.get("measurements", {})
    construction = m.get("execution", {}).get("construction", {}) or {}
    legs = construction.get("legs_ms", {}) or {}
    executor = m.get("execution", {}).get("executor", {}) or {}
    memory = next(iter((record.get("report", {}).get("measured") or [{}])[0].get("memory") or [{}]), {})
    store = memory.get("store") or {}
    accepted, started, settled = (record.get(k) or 0 for k in ("accepted_at_ms", "started_at_ms", "settled_at_ms"))
    execution = record.get("execution_ms") or 0
    prepare = construction.get("prepare_ms") or 0
    load = stage(record, "answering / loading model")
    answering = stage(record, "answering")
    store_gbps = store.get("gbps")
    disk = ceiling.get("disk_gbps")
    return {
        "label": label,
        "run": record.get("name"),
        "status": record.get("status"),
        "wait_s": round((started - accepted) / 1000, 1) if started and accepted else None,
        "executor_s": round((executor.get("ms") or 0) / 1000, 1),
        "prepare_s": round(prepare / 1000, 1),
        "factory_s": round(legs.get("construct.factory", 0) / 1000, 1),
        "fill_s": round(legs.get("construct.fill", 0) / 1000, 1),
        "leaves_s": round(legs.get("register.leaves", 0) / 1000, 1),
        "routes_s": round(sum(v for k, v in legs.items() if k.startswith("register.") and k.endswith(".routes")) / 1000, 1),
        "register_s": round(sum(v for k, v in legs.items() if k.startswith("register.") and k.endswith(".load")) / 1000, 1),
        "answer_s": round(answering / 1000, 1),
        "load_s": round(load / 1000, 1),
        "execution_s": round(execution / 1000, 1),
        "store_gb": round((store.get("bytes") or 0) / 1e9, 2),
        "store_gbps": round(store_gbps, 2) if store_gbps else None,
        "disk_ceiling_gbps": round(disk, 2) if disk else None,
        "h2d_ceiling_gbps": round(ceiling["h2d_gbps"], 1) if ceiling.get("h2d_gbps") else None,
        "peak_gb": round((m.get("memory", {}).get("peak_reserved_bytes") or 0) / 1e9, 1),
        "text": ((record.get("result") or {}).get("text") or "")[:160],
    }


ceiling = json.load(open(sys.argv[1]))
bars = json.load(open(sys.argv[2]))
rows, failed = [], False
for pair in sys.argv[3:]:
    label, path = pair.split("=", 1)
    try:
        record = json.load(open(path))
    except (OSError, ValueError) as exc:
        rows.append({"label": label, "status": f"unrecorded: {exc}"})
        continue
    rows.append(row(label, record, ceiling))
for r in rows:
    print(json.dumps(r))
cols = ["label", "run", "status", "wait_s", "executor_s", "prepare_s", "factory_s", "fill_s", "leaves_s", "routes_s",
        "register_s", "answer_s", "load_s", "execution_s", "store_gb", "store_gbps", "disk_ceiling_gbps", "peak_gb"]
print("| " + " | ".join(cols) + " |")
print("|" + "---|" * len(cols))
for r in rows:
    print("| " + " | ".join("" if r.get(c) is None else str(r.get(c)) for c in cols) + " |")
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
            ok, said = r["prepare_s"] <= bar["prepare_s"], f"construction {r['prepare_s']} s, bar {bar['prepare_s']} s"
        else:
            limit = round(bar["x"] * r["store_gb"] / disk + bar["fixed_s"], 1) if disk else None
            ok = limit is not None and r["execution_s"] <= limit
            said = (f"execution {r['execution_s']} s, bar {limit} s ({bar['x']} x {r['store_gb']} GB at the disk's "
                    f"{disk:.2f} GB/s + {bar['fixed_s']} s)")
        print(f"{'PASS' if ok else 'FAIL'} {arm}-{case}: {said}{'' if judged else ' (not judged)'}")
        failed |= judged and not ok
sys.exit(1 if failed else 0)
