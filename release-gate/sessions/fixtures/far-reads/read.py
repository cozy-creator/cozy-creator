"""far-reads' readings (cl-1216), off this runner's own files and sockets.

  read.py landed <project>            the newest run's video here: "<birth> <last write> <bytes> <path>" (unix seconds).
  read.py ramp <samples> <t0> <t1>    how its bytes came between t0 and t1, from `ss` samples: a JSON line.
  read.py wire <before> <after>       the bytes the pod's connections received between two `ss` snapshots.
  read.py trim <samples> <t0> <t1>    the sample file cut to that read, a second each side.

A sample file is `@ <unix seconds>` then `ss -tinH dst <pod>` as it printed, again and again."""

import json
import os
import re
import subprocess
import sys


def sockets(text: str) -> dict[str, tuple[int, float]]:
    """Each connection by its local port: the bytes it received and its round trip, ms."""
    found, port = {}, None
    for line in text.splitlines():
        if line and not line[0].isspace():
            words = line.split()
            port = words[3].rsplit(":", 1)[-1] if len(words) > 4 else None
        elif port:
            got, rtt = re.search(r"bytes_received:(\d+)", line), re.search(r"\brtt:([\d.]+)/", line)
            found[port] = (int(got[1]) if got else 0, float(rtt[1]) if rtt else 0.0)
    return found


def samples(path: str) -> list[tuple[float, dict[str, tuple[int, float]]]]:
    out = []
    for block in open(path, errors="replace").read().split("@ ")[1:]:
        at, _, rest = block.partition("\n")
        out.append((float(at), sockets(rest)))
    return out


def landed(project: str) -> None:
    listed = subprocess.run(["cozy", "--json", "--full", "run", "ls", "--limit", "1"], cwd=project, capture_output=True, text=True)
    runs = json.loads(listed.stdout)["runs"]
    digest = runs[0]["output"][0]["sha256"].removeprefix("sha256:")
    path = os.path.join(project, "files", f"{digest}.mp4")
    birth = subprocess.run(["stat", "-c", "%.9W", path], capture_output=True, text=True, check=True).stdout.strip()
    at = os.stat(path)
    print(birth, f"{at.st_mtime:.9f}", at.st_size, path)


def ramp(path: str, t0: float, t1: float) -> None:
    seen = samples(path)
    before = next((s for at, s in reversed(seen) if at <= t0), {})
    marks = {"64 KiB": 64 << 10, "1 MiB": 1 << 20}
    rows, new, rtts = [], set(), []
    for at, now in seen:
        if at < t0 or at > t1 + 0.5:
            continue
        came = sum(got - before.get(port, (0, 0))[0] for port, (got, _) in now.items() if got > before.get(port, (0, 0))[0])
        new |= {port for port, (got, _) in now.items() if port not in before and got > 0}
        rtts += [rtt for _, rtt in now.values() if rtt]
        rows.append((at - t0, came))
    total = rows[-1][1] if rows else 0
    marks |= {"a quarter": total // 4, "half": total // 2, "all but 64 KiB": max(total - (64 << 10), 0)}
    carried = {port for port, (got, _) in (seen[-1][1] if seen else {}).items() if got - before.get(port, (0, 0))[0] > 64 << 10}
    rtt = sorted(rtts)[len(rtts) // 2] if rtts else 0.0
    reached = {name: next((round(t, 2) for t, came in rows if came >= most), None) for name, most in marks.items()}
    print(json.dumps({"seconds": round(t1 - t0, 2), "bytes_on_its_connections": total, "connections": len(carried), "new": len(new & carried) if carried else len(new),
                      "rtt_ms": round(rtt, 1), "reached_s": reached,
                      "round_trips_to_64_kib": round(reached["64 KiB"] * 1000 / rtt, 1) if rtt and reached["64 KiB"] is not None else None}))


def trim(path: str, t0: float, t1: float) -> None:
    blocks = open(path, errors="replace").read().split("@ ")[1:]
    kept = [block for block in blocks if t0 - 1 <= float(block.partition("\n")[0]) <= t1 + 1]
    open(path, "w").write("".join(f"@ {block}" for block in kept))


def wire(before: str, after: str) -> None:
    was, now = sockets(open(before).read()), sockets(open(after).read())
    print(sum(max(got - was.get(port, (0, 0))[0], 0) for port, (got, _) in now.items()))


if __name__ == "__main__":
    match sys.argv[1:]:
        case ["landed", project]:
            landed(project)
        case ["ramp", path, t0, t1]:
            ramp(path, float(t0), float(t1))
        case ["wire", before, after]:
            wire(before, after)
        case ["trim", path, t0, t1]:
            trim(path, float(t0), float(t1))
        case _:
            sys.exit(__doc__)
