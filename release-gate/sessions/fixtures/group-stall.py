"""stall.py MODE [CONT_S]: what a group follower suffers on its next formation, as a GPU fault would do it.
  kill  SIGKILL the follower as soon as it exists, before it joins
  stop  SIGSTOP it the moment it joins rank 0's store (its TCP connection to a port rank 0 listens on)
  stop  with CONT_S: and SIGCONT it CONT_S s later
The follower is the executor whose parent and grandparent are executors (zygote -> GPU 0 -> follower).
Prints what it did with timestamps; every executor's state and the GPUs follow at the end."""

import os
import signal
import subprocess
import sys
import time
from pathlib import Path


def executors() -> dict[int, int]:
    found = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            if (
                b"cozy_runtime.internal.executor"
                not in Path(f"/proc/{d}/cmdline").read_bytes()
            ):
                continue
            status = Path(f"/proc/{d}/status").read_text().splitlines()
            ppid = int(
                next(line for line in status if line.startswith("PPid")).split()[1]
            )
        except (OSError, StopIteration):
            continue
        found[int(d)] = ppid
    return found


def follower(seen: set[int]) -> tuple[int, int] | None:
    ex = executors()
    for pid, ppid in ex.items():
        if pid not in seen and ppid in ex and ex[ppid] in ex:
            return pid, ppid
    return None


def sockets(pid: int) -> set[str]:
    inodes = set()
    try:
        for fd in os.listdir(f"/proc/{pid}/fd"):
            try:
                link = os.readlink(f"/proc/{pid}/fd/{fd}")
            except OSError:
                continue
            if link.startswith("socket:["):
                inodes.add(link[8:-1])
    except OSError:
        pass
    return inodes


def tcp() -> list[tuple[str, str, str, str]]:
    rows = []
    for name in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            for line in Path(name).read_text().splitlines()[1:]:
                f = line.split()
                rows.append(
                    (f[1].rsplit(":", 1)[1], f[2].rsplit(":", 1)[1], f[3], f[9])
                )  # local port, remote port, state, inode
        except OSError:
            pass
    return rows


def joined(pid: int, parent: int) -> bool:
    mine, theirs = sockets(pid), sockets(parent)
    rows = tcp()
    listening = {lp for lp, _, st, ino in rows if st == "0A" and ino in theirs}
    return any(
        st == "01" and ino in mine and rp in listening for _, rp, st, ino in rows
    )


def main() -> None:
    mode = sys.argv[1]
    cont = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
    seen = set(executors())
    began = time.time()

    def say(what: str) -> None:
        print(
            f"{time.strftime('%H:%M:%S')} +{time.time() - began:.3f}s {what}",
            flush=True,
        )

    say(f"waiting for a new follower ({len(seen)} executors now)")
    while (found := follower(seen)) is None:
        if time.time() - began > 600:
            say("no follower in 10 min")
            return
        time.sleep(0.005)
    pid, parent = found
    say(f"follower {pid} of GPU 0 {parent}")
    if mode == "kill":
        os.kill(pid, signal.SIGKILL)
        say(f"SIGKILL {pid} before it joined")
    else:
        while not joined(pid, parent):
            if not os.path.exists(f"/proc/{pid}"):
                say("follower gone before joining")
                return
            time.sleep(0.001)
        os.kill(pid, signal.SIGSTOP)
        stopped = time.time()
        say(f"SIGSTOP {pid}: joined GPU 0's store")
        if cont:
            time.sleep(max(cont - (time.time() - stopped), 0))
            os.kill(pid, signal.SIGCONT)
            say(f"SIGCONT {pid} after {time.time() - stopped:.1f} s")
    time.sleep(1)
    say(f"executors: {executors()}")
    apps = [
        "nvidia-smi",
        "--query-compute-apps=pid,gpu_bus_id,used_memory",
        "--format=csv,noheader",
    ]
    say(
        subprocess.run(apps, capture_output=True, text=True, check=False).stdout.strip()
        or "no GPU processes"
    )


if __name__ == "__main__":
    main()
