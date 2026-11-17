#!/usr/bin/env python3
"""stale_parent: cozy's process trees (`platform::process::tree`, which ends a let-go view's provider and a project's processes)
never take a process that started before its supposed parent.

Windows keeps the parent id a process was created with. A, a short-lived parent, starts C, which lives on, and exits. A new process
T then gets A's pid, so C's recorded parent is T's pid, but C started before T: ending T's tree must leave C running. And on every
OS, ending a shell's tree ends the child it started. (On Linux and macOS a process's parent follows it when its parent exits, so the
collision is Windows'.)
"""

from __future__ import annotations

import argparse
import gc
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import cast

ROUNDS = 6
PER_ROUND = 150


def probe(binary: Path, pid: int) -> list[int]:
    """The pids `process-tree kill pid` ended: `pid` and what `platform::process::tree` counted as its."""
    done = subprocess.run([str(binary), "kill", str(pid)], capture_output=True, text=True, timeout=120)
    assert done.returncode == 0, f"process-tree kill {pid}: {done.stderr[-400:]}"
    return cast(list[int], json.loads(done.stdout.strip().splitlines()[-1])["tree"])


def own_child(binary: Path, shell: list[str], facts: dict[str, object], failures: list[str]) -> None:
    """A shell's tree ends with the child it started."""
    target = subprocess.Popen(shell, creationflags=0x08000000 if sys.platform == "win32" else 0)
    time.sleep(2)
    tree = facts["own_tree"] = probe(binary, target.pid)
    target.wait(timeout=60)
    if len(tree) < 2:
        failures.append(f"ending {target.pid} didn't take the child it started: {tree}")


def windows(binary: Path, facts: dict[str, object], failures: list[str]) -> None:
    import ctypes
    from ctypes import wintypes

    kernel = getattr(ctypes, "WinDLL")("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    synchronize_terminate = 0x00100000 | 0x0001
    no_window = 0x08000000

    def held(pid: int) -> int:
        handle = kernel.OpenProcess(synchronize_terminate, False, pid)
        assert handle, f"open process {pid}: {getattr(ctypes, "get_last_error")()}"
        return int(handle)

    def running(handle: int) -> bool:
        return int(kernel.WaitForSingleObject(handle, 0)) == 0x102  # WAIT_TIMEOUT

    # A starts C, which lives on, prints its pid and exits.
    spawn_c = ("import subprocess, sys; c = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(900)'], "
               f"creationflags={no_window}); print(c.pid, flush=True)")
    a = subprocess.Popen([sys.executable, "-c", spawn_c], stdout=subprocess.PIPE, text=True, creationflags=no_window)
    assert a.stdout is not None
    c_pid = int(a.stdout.readline())
    c = held(c_pid)
    a.wait(timeout=60)
    a_pid = a.pid
    del a
    gc.collect()
    facts.update({"a_pid": a_pid, "c_pid": c_pid})

    # Processes until one gets A's pid: T.
    target: subprocess.Popen[bytes] | None = None
    spawned = 0
    for _ in range(ROUNDS):
        batch: list[subprocess.Popen[bytes]] = []
        for _ in range(PER_ROUND):
            started = subprocess.Popen(["ping", "-n", "900", "127.0.0.1"], stdout=subprocess.DEVNULL, creationflags=no_window)
            spawned += 1
            if started.pid == a_pid:
                target = started
                break
            batch.append(started)
        for other in batch:
            other.kill()
            other.wait()
        del batch
        gc.collect()
        if target is not None:
            break
    facts["spawned"] = spawned
    if target is None:
        kernel.TerminateProcess(c, 1)
        failures.append(f"no process got A's pid {a_pid} in {spawned} starts: the collision wasn't made")
        return
    tree = facts["tree"] = probe(binary, target.pid)
    target.wait(timeout=60)
    facts["c_survived"] = running(c)
    if not facts["c_survived"] or c_pid in tree:
        failures.append(f"ending T ({target.pid}) ended C ({c_pid}), which started before T and only records T's pid as its parent: {tree}")
    kernel.TerminateProcess(c, 1)
    own_child(binary, ["cmd.exe", "/c", "ping -n 900 127.0.0.1 > NUL"], facts, failures)


def unix(binary: Path, facts: dict[str, object], failures: list[str]) -> None:
    own_child(binary, ["sh", "-c", "sleep 900 & wait"], facts, failures)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    binary: Path = args.probe
    if sys.platform == "win32" and not binary.exists():
        binary = binary.with_suffix(".exe")
    facts: dict[str, object] = {"os": sys.platform, "pid": os.getpid()}
    failures: list[str] = []
    try:
        (windows if sys.platform == "win32" else unix)(binary, facts, failures)
    finally:
        (args.out / "stale-parent.json").write_text(json.dumps(facts, indent=2, default=str))
    print(f"stale parent: {json.dumps(facts)}", flush=True)
    assert not failures, "stale parent: " + "; ".join(failures)


if __name__ == "__main__":
    main()
