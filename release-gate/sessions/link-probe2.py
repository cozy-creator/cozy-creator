"""link-probe2's readings per stage (tfs-300): each owner's MB/s and round trips, TensorD's disk writes and CPU, the page cache's
dirty bytes, each disk's write rate and busy share, and the Hub's control latency (p50/p95/max of connect, TLS and total)."""

import json
import statistics
import sys
from pathlib import Path


def quantile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    values = sorted(values)
    return round(values[min(len(values) - 1, int(q * len(values)))], 4)


def main() -> None:
    out = Path(sys.argv[1])
    rows = [json.loads(line) for line in (out / "probe2.jsonl").read_text().splitlines() if line.startswith("{")]
    stages: dict[str, dict[str, list[float]]] = {}
    for last, row in zip(rows, rows[1:]):
        dt = row["t"] - last["t"]
        if dt <= 0:
            continue
        s = stages.setdefault(row["stage"], {})

        def add(key: str, value: float) -> None:
            s.setdefault(key, []).append(value)

        for name in ("weights", "install", "probe"):
            add(f"{name}_mb_s", max(0, row[name]["bytes"] - last[name]["bytes"]) / dt / 1e6)
            add(f"{name}_n", row[name]["n"])
            if row[name]["rcv_rtt_us"]:
                add(f"{name}_rcv_rtt_ms", statistics.median(row[name]["rcv_rtt_us"]) / 1e3)
            if row[name]["rtt_us"]:
                add(f"{name}_rtt_ms", statistics.median(row[name]["rtt_us"]) / 1e3)
        add("link_mb_s", max(0, row["link_bytes"] - last["link_bytes"]) / dt / 1e6)
        add("tensord_write_mb_s", max(0, row["tensord"]["write_bytes"] - last["tensord"]["write_bytes"]) / dt / 1e6)
        add("tensord_cores", max(0, row["tensord"]["cpu_ticks"] - last["tensord"]["cpu_ticks"]) / dt / 100)
        add("dirty_mb", row["dirty_kb"] / 1e3)
        add("writeback_mb", row["writeback_kb"] / 1e3)
        for disk, (written, busy) in row["disks"].items():
            before = last["disks"].get(disk)
            if before:
                add(f"disk_{disk}_write_mb_s", max(0, written - before[0]) / dt / 1e6)
                add(f"disk_{disk}_busy", max(0, busy - before[1]) / dt / 1e3)
    summary: dict[str, dict[str, object]] = {}
    for stage, values in stages.items():
        summary[stage] = {"s": len(values.get("link_mb_s", []))}
        for key, xs in values.items():
            if key.endswith(("_n",)):
                summary[stage][key] = max(xs)
            elif key in ("dirty_mb", "writeback_mb") or key.endswith("_busy"):
                summary[stage][key] = {"p50": quantile(xs, 0.5), "max": round(max(xs), 2)}
            else:
                summary[stage][key] = {"p50": quantile(xs, 0.5), "p90": quantile(xs, 0.9), "max": round(max(xs), 2)}
    control: dict[str, list[list[float]]] = {}
    versions: set[str] = set()
    for line in (out / "control.txt").read_text(errors="replace").splitlines():
        f = line.split()
        if len(f) >= 7:
            try:
                control.setdefault(f[1], []).append([float(f[2]), float(f[3]), float(f[4])])
                versions.add(f[5])
            except ValueError:
                pass
    for stage, samples in control.items():
        summary.setdefault(stage, {})["control"] = {
            "n": len(samples),
            **{name: {"p50": quantile([x[i] for x in samples], 0.5), "p95": quantile([x[i] for x in samples], 0.95), "max": round(max(x[i] for x in samples), 4)}
               for i, name in enumerate(("connect_s", "tls_s", "total_s"))},
        }
    print(json.dumps({"hub_http_versions": sorted(versions), "sock_diag": sorted({str(r.get("sock_diag")) for r in rows}), "stages": summary}))


main()
