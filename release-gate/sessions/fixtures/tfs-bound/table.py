"""tfs-bound's table (sessions/tfs-bound.sh): every load against its bound, per arm.

    python3 -I table.py <session out dir> <arm>...

The bound of bytes read from disk onto the card is max(bytes / disk, bytes / H2D); from RAM, bytes / H2D; into pinned RAM,
bytes / disk. `disk` is TensorFS's own ceiling (ceiling.json), `disk max` the fastest O_DIRECT case of the disk grid
(disk.json), H2D TensorFS's pinned ceiling. `x` is observed / bound: 1.0 is the bound.
"""

import json
import os
import sys
from typing import TypeAlias

Obj: TypeAlias = dict[str, object]


def obj(x: object) -> Obj:
    return x if isinstance(x, dict) else {}


def num(x: object) -> float:
    return float(x) if isinstance(x, (int, float)) and not isinstance(x, bool) else 0.0


def arr(x: object) -> list[object]:
    return x if isinstance(x, list) else []


def first(x: object) -> Obj:
    return obj(x[0]) if isinstance(x, list) and x else {}


def load(path: str) -> Obj:
    try:
        return obj(json.load(open(path)))
    except (OSError, ValueError):
        return {}


out, arms = sys.argv[1], sys.argv[2:]
ceiling, disk = load(f"{out}/ceiling.json"), load(f"{out}/disk.json")
d, h = num(ceiling.get("disk_gbps")), num(ceiling.get("h2d_gbps"))
cases = [obj(c) for c in arr(disk.get("cases")) if isinstance(c, dict)]
dmax = max((num(c.get("gb_s")) for c in cases if c.get("direct")), default=0.0)
print(f"ceilings: disk {d:.2f} GB/s ({'O_DIRECT' if ceiling.get('disk_direct') else 'buffered'}), pinned H2D {h:.1f} GB/s "
      f"({ceiling.get('streams')} streams); disk max {dmax:.2f} GB/s (grid below); {ceiling.get('cpus')} CPUs, memcpy GB/s by "
      f"threads {json.dumps(ceiling.get('memcpy_gbps'))}")
print()
print("| disk case | threads | MiB | GB | GB/s |")
print("|---|---|---|---|---|")
for c in cases + ([obj(disk.get("direct_again"))] if disk.get("direct_again") else []):
    kind = "O_DIRECT" if c.get("direct") else "buffered, cold"
    print(f"| {kind} | {c.get('threads')} | {c.get('request_mib')} | {c.get('gb')} | {c.get('gb_s')} |")
print()


def bound(gb: float, cold: bool, onto_card: bool, disk_rate: float) -> float:
    parts = []
    if cold and disk_rate:
        parts.append(gb / disk_rate)
    if onto_card and h:
        parts.append(gb / h)
    return max(parts) if parts else 0.0


rows: list[list[str]] = []


def add(arm: str, case: str, gb: float, s: float, cold: bool, onto_card: bool, note: str = "") -> None:
    if not gb or not s:
        rows.append([arm, case, "-", "-", "-", "-", "-", "-", note or "no measure"])
        return
    b, braw = bound(gb, cold, onto_card, d), bound(gb, cold, onto_card, dmax)
    rows.append([arm, case, f"{gb:.2f}", f"{s:.2f}", f"{gb / s:.2f}", f"{b:.2f}", f"{s / b:.2f}" if b else "-",
                 f"{s / braw:.2f}" if braw else "-", note])


try:
    dd = " ".join(line.split(",")[-1].strip() for line in open(f"{out}/dd.txt") if "copied" in line)
except OSError:
    dd = ""
print(f"dd iflag=direct of the store's chunks: {dd or '-'}")
for arm in arms:
    e = load(f"{out}/{arm}-ceiling-empty.json")
    if e:
        print(f"{arm}: the disk probe of a TensorFS home with no weights: {e.get('disk_gbps', e.get('error'))} GB/s")
print()
for arm in arms:
    for name, label in (("fill", "qwen-image-2"), ("h3-fill", "h3 transformer")):
        f = load(f"{out}/{arm}-{name}.json")
        if not f:
            continue
        busy = "overlapped " if f.get("busy_before") or f.get("busy_after") else ""
        if f.get("error"):
            rows.append([arm, f"{label} fill", "-", "-", "-", "-", "-", "-", f"{busy}{f['error']}"[:120]])
            continue
        for temp, cold in (("cold", True), ("warm", False)):
            r = obj(f.get(temp))
            note = (f"{busy}{r.get('blocks')} blocks, {r.get('alone')} alone, O_DIRECT {r.get('o_direct_gb')} GB, through the cache "
                    f"{r.get('through_cache_gb')} GB, reads {json.dumps(r.get('rows'))}")
            if temp == "cold":
                note += f", cached before {f.get('cached_before_cold')}, after {f.get('cached_after_cold')}"
            add(arm, f"{label} fill {'disk' if cold else 'RAM'} -> card", num(r.get("gb")), num(r.get("s")), cold, True, note)
    f = load(f"{out}/{arm}-policy-full.json")
    if f.get("error"):
        rows.append([arm, "policy: cache filled first", "-", "-", "-", "-", "-", "-", str(f["error"])[:160]])
    elif f:
        for key, case, cold in (("cold", "disk -> card", True), ("again", "the same again", False)):
            r = obj(f.get(key))
            note = f"O_DIRECT {r.get('o_direct_gb')} GB, through the cache {r.get('through_cache_gb')} GB"
            if key == "cold":
                note += f"; cached after {f.get('cached_after_cold')}; filler {json.dumps(f.get('filler'))}; memory {json.dumps(f.get('memory_before'))}"
            add(arm, f"policy, cache filled first: {case}", num(r.get("gb")), num(r.get("s")), cold, True, note)
    f = load(f"{out}/{arm}-holes.json")
    if f.get("error"):
        rows.append([arm, "holes: 4 MiB of every 4th chunk cached, memory held", "-", "-", "-", "-", "-", "-", str(f["error"])[:160]])
    elif f:
        r, b = obj(f.get("load")), obj(f.get("bytes"))
        add(arm, "holes: 4 MiB of every 4th chunk cached, memory held: disk + RAM -> card", num(r.get("gb")), num(r.get("s")), True, True,
            f"O_DIRECT {r.get('o_direct_gb')} GB, through the cache {r.get('through_cache_gb')} GB; cached before {f.get('cached_before')}, after the hold {f.get('cached_after_hold')}, "
            f"held {f.get('held_gb')} GB; bytes: {b.get('blocks')} blocks, {b.get('gb')} GB, wrong {b.get('wrong_blocks')}")
    paged = [("", f"{out}/{arm}-h3-paged.json")]
    i = 1
    while os.path.exists(f"{out}/{arm}-paged-sweep-{i}.json"):
        paged.append((f"sweep {i} ", f"{out}/{arm}-paged-sweep-{i}.json"))
        i += 1
    for tag, path in paged:
        p = load(path)
        env = " ".join(f"{k}={v}" for k, v in obj(p.get("env")).items())
        tag = f"{tag}[{env}] " if env else tag
        if p and not p.get("error"):
            busy = "overlapped " if p.get("busy_before") or p.get("busy_after") else ""
            for key, case, cold, card in (("read_host", "h3 disk -> pinned (read_host, 8 readers)", True, False),
                                          ("read_ahead", "h3 disk -> pinned pages (read_ahead)", True, False),
                                          ("from_pages", "h3 pinned pages -> card", False, True)):
                r = obj(p.get(key))
                add(arm, tag + case, num(r.get("gb")), num(r.get("s")), cold, card,
                    busy + json.dumps({k: v for k, v in r.items() if k not in ("gb", "s", "gb_s")})[:200])
        elif p:
            rows.append([arm, tag + "h3 paged", "-", "-", "-", "-", "-", "-", str(p.get("error"))[:120]])
    i = 1
    while os.path.exists(f"{out}/{arm}-sweep-{i}.json"):
        f = load(f"{out}/{arm}-sweep-{i}.json")
        env = " ".join(f"{k}={v}" for k, v in obj(f.get("env")).items())
        busy = "overlapped " if f.get("busy_before") or f.get("busy_after") else ""
        for temp, cold in (("cold", True), ("warm", False)):
            r = obj(f.get(temp))
            add(arm, f"sweep {i} {'disk' if cold else 'RAM'} [{env}]", num(r.get("gb")), num(r.get("s")), cold, True,
                busy + (str(f.get("error"))[:100] if f.get("error") else f"reads {json.dumps(r.get('rows'))}"))
        i += 1
    for run, cold in (("qimg-cold", True), ("qimg-warm", False), ("sdxl-new", False), ("sdxl-warm", False), ("h3-cold", True), ("h3-warm", False)):
        rec = load(f"{out}/{arm}-{run}.json")
        if not rec:
            continue
        m = obj(rec.get("measurements"))
        moved = obj(obj(first(obj(obj(m.get("memory")).get("stats")).get("varena")).get("stats")).get("moved"))
        stages = [obj(x) for x in arr(first(obj(rec.get("report")).get("measured")).get("stages")) if isinstance(x, dict)]
        said = ", ".join(f"{x.get('name')} {num(x.get('ms')) / 1000:.1f} s" for x in stages if num(x.get("ms")) >= 100)
        head = f"{rec.get('name')} {rec.get('status')}, executing {num(rec.get('execution_ms')) / 1000:.1f} s; {said}"
        if not moved:
            rows.append([arm, f"run {run}", "-", "-", "-", "-", "-", "-", head + " (no fill record: a parent call's)"])
            continue
        for origin, case, from_disk in (("store", "store -> card", cold), ("host", "pinned pages -> card", False)):
            f = obj(moved.get(origin))
            gb, s_ = num(f.get("bytes")) / 1e9, num(f.get("ns")) / 1e9
            if gb:
                add(arm, f"run {run} {case} ({int(num(f.get('fills')))} fills)", gb, s_, from_disk, True, head if origin == "store" else "")

print("| arm | case | GB | s | GB/s | bound s | x (disk ceiling) | x (disk max) | notes |")
print("|---|---|---|---|---|---|---|---|---|")
for cells in rows:
    print("| " + " | ".join(cells) + " |")
