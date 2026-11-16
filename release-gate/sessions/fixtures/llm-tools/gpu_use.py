"""cl-1100's GPU check (llm-tools.sh): the machine drawer's GPU use held to nvidia-smi on the same pod during renders.

  python3 gpu_use.py <smi.csv> <drawer.jsonl> <heard.jsonl>

smi.csv: `<pod unix ms>,<index>,<utilization.gpu %>,<memory.used MiB>[,<memory.reserved MiB>]`, nvidia-smi's every second on the pod.
drawer.jsonl: what the drawer showed twice a second (`machine-details: GPU use` with GPU_LOG): each GPU's busy meter and memory words, and
the last reading the page heard. heard.jsonl: every reading the page heard (TensorD's `GpuUse`, stamped by the pod's clock).
Each heard reading is paired with the pod's nvidia-smi row of that GPU nearest in time (within 700 ms): both are NVML, so busy within
10 points and memory within 200 MB (cl-1100). And what the drawer showed is the reading it heard last. Prints a table and PASS/FAIL lines.
"""
import json
import statistics
import sys
import time
from typing import TypedDict

MB = 10**6
BUSY_POINTS, MEMORY_BYTES, NEAR_MS, SHARE = 10, 200 * MB, 700, 0.9


class Gpu(TypedDict):
    index: int
    busy_percent: int
    memory_used_bytes: int


class Reading(TypedDict, total=False):
    """TensorD's `GpuUse`, as the page heard it."""
    at_unix_ms: int
    heard: int
    gpus: list[Gpu]


class Shown(TypedDict):
    index: int
    busy: int
    memory: str | None


class Look(TypedDict, total=False):
    """What the drawer showed, and the last reading the page heard."""
    at: int
    last: Reading | None
    shown: list[Shown]


# An nvidia-smi row: pod unix ms, busy %, memory used and reserved (bytes; reserved None where it doesn't say).
Row = tuple[int, int, int, int | None]
# A reading paired with nvidia-smi's: pod ms, Cozy's busy and memory, then nvidia-smi's busy, memory and reserved.
Pair = tuple[int, int, int, int, int, int | None]


def shown_bytes(n: int) -> str:
    """web/app/src/panels/format.ts `bytes`."""
    units, value, unit = ["B", "KB", "MB", "GB", "TB", "PB"], float(n), 0
    while value >= 1024 and unit < len(units) - 1:
        value, unit = value / 1024, unit + 1
    if unit == 0:
        return f"{n}B"
    text = f"{value:.1f}".removesuffix(".0") if value < 10 else f"{value:.0f}"
    return f"{text}{units[unit]}"


def readings(path: str) -> list[Reading]:
    said: list[Reading] = [json.loads(line) for line in open(path) if line.strip().startswith("{")]
    return said


def looks(path: str) -> list[Look]:
    said: list[Look] = [json.loads(line) for line in open(path) if line.strip().startswith("{")]
    return said


def nearest(rows: list[Row], at: int) -> Row | None:
    """nvidia-smi's row nearest `at`, within NEAR_MS."""
    if not rows:
        return None
    row = min(rows, key=lambda r: abs(r[0] - at))
    return row if abs(row[0] - at) <= NEAR_MS else None


def of(reading: Reading | None, index: int) -> Gpu | None:
    return next((gpu for gpu in (reading or {}).get("gpus") or [] if gpu["index"] == index), None)


def main() -> int:
    smi_path, drawer_path, heard_path = sys.argv[1:4]
    smi: dict[int, list[Row]] = {}
    for line in open(smi_path):
        f = [x.strip() for x in line.split(",")]
        if len(f) < 4 or not f[0].isdigit():
            continue
        try:
            row_at, row_index, row_busy, row_used = int(f[0]), int(f[1]), int(f[2]), int(float(f[3])) * 2**20
        except ValueError:
            continue
        row_reserved = int(float(f[4])) * 2**20 if len(f) > 4 and f[4].replace(".", "").isdigit() else None
        smi.setdefault(row_index, []).append((row_at, row_busy, row_used, row_reserved))
    heard, drawer = readings(heard_path), looks(drawer_path)
    fails: list[str] = []
    table: list[str] = []
    if not smi:
        fails.append("no nvidia-smi rows")
    if not heard:
        fails.append("the page heard no reading")
    pairs: dict[int, list[Pair]] = {}
    for reading in heard:
        at = reading.get("at_unix_ms") or 0
        for gpu in reading.get("gpus") or []:
            near = nearest(smi.get(gpu["index"]) or [], at)
            if near:
                pairs.setdefault(gpu["index"], []).append((at, gpu["busy_percent"], gpu["memory_used_bytes"], near[1], near[2], near[3]))
    print("| GPU | pairs | busy: Cozy max | busy: nvidia-smi max | busy within 10 points | busy Δ median / max | memory within 200 MB | memory Δ median / max (MB) |")
    print("|---|---|---|---|---|---|---|---|")
    for index in sorted(set(smi) | set(pairs)):
        paired = pairs.get(index, [])
        if not paired:
            fails.append(f"GPU {index}: no reading paired with nvidia-smi within {NEAR_MS} ms")
            continue
        db = [abs(p[1] - p[3]) for p in paired]
        dm = [abs(p[2] - p[4]) for p in paired]
        busy_in = sum(d <= BUSY_POINTS for d in db) / len(paired)
        mem_in = sum(d <= MEMORY_BYTES for d in dm) / len(paired)
        print(f"| {index} | {len(paired)} | {max(p[1] for p in paired)}% | {max(p[3] for p in paired)}% | {busy_in:.0%} | {statistics.median(db):.0f} / {max(db)} |"
              f" {mem_in:.0%} | {statistics.median(dm) / MB:.0f} / {max(dm) / MB:.0f} |")
        with_reserve = [abs(p[2] - (p[4] + r)) for p in paired if (r := p[5]) is not None]
        if mem_in < SHARE and with_reserve:
            print(f"|   | GPU {index}: Cozy's memory against nvidia-smi's used + reserved: Δ median {statistics.median(with_reserve) / MB:.0f} MB |||||||")
        if busy_in < SHARE:
            fails.append(f"GPU {index}: busy within {BUSY_POINTS} points of nvidia-smi in {busy_in:.0%} of {len(paired)} readings")
        if mem_in < SHARE:
            fails.append(f"GPU {index}: memory within 200 MB of nvidia-smi in {mem_in:.0%} of {len(paired)} readings")
    if not any(p[1] >= 50 for each in pairs.values() for p in each):
        fails.append("no GPU read 50% busy or more: the renders weren't watched")
    # Side by side, a row each 5 s: what the drawer showed, the reading it showed, nvidia-smi then.
    print("\n| pod time (UTC) | GPU | drawer busy | drawer memory | Cozy busy | Cozy memory (MiB) | nvidia-smi busy | nvidia-smi memory (MiB) |")
    print("|---|---|---|---|---|---|---|---|")
    last_at = 0
    for look in drawer:
        last = look.get("last")
        at = (last or {}).get("at_unix_ms") or 0
        if at - last_at < 5000:
            continue
        last_at = at
        for shown in look.get("shown") or []:
            gpu = of(last, shown["index"])
            near = nearest(smi.get(shown["index"]) or [], at)
            if gpu and near:
                clock = time.strftime("%H:%M:%S", time.gmtime(at / 1000))
                table.append(f"| {clock} | {shown['index']} | {shown['busy']}% | {shown['memory']} | {gpu['busy_percent']}% |"
                             f" {gpu['memory_used_bytes'] / 2**20:.0f} | {near[1]}% | {near[2] / 2**20:.0f} |")
    print("\n".join(table))
    # The drawer shows the reading it heard last.
    agree = total = 0
    for look in drawer:
        for shown in look.get("shown") or []:
            gpu = of(look.get("last"), shown["index"])
            if not gpu:
                continue
            total += 1
            agree += shown["busy"] == gpu["busy_percent"] and (shown["memory"] or "").startswith(shown_bytes(gpu["memory_used_bytes"]))
    if total == 0:
        fails.append("the drawer showed no GPU use")
    elif agree / total < SHARE:
        fails.append(f"the drawer showed the reading it heard in {agree / total:.0%} of {total} looks")
    print(f"\nthe drawer showed the reading it heard last in {agree} of {total} looks")
    for fail in fails:
        print(f"FAIL {fail}")
    if not fails:
        print("PASS the drawer's GPU use agrees with nvidia-smi within 10 points and 200 MB")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
