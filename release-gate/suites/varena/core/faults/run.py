#!/usr/bin/env python3
"""Varena fault suite. A fault must end in a clean typed error or correct bytes, with the process and
GPU recovered, no hang, no corruption and no Xid. A hang is a failure, so progress is watched.

Run on a rental over `cozy machine ssh`, never on a laptop or through `cozy run`: faults are deliberate.

  PYTHONPATH=<wheel dir> FAULT_PROVIDER=<provider.so> python3 run.py --out <dir> [cell ...]

Build provider.so from native-tensor-export/source_provider.c against the wheel's .native directory.
The rules that fault nothing are in ../../programs/core.py, a program like this one (xs-174).
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import TypeAlias, TypedDict, cast

HERE = Path(__file__).resolve().parent
PROBLEM_OUTCOMES = {"corrupt", "device_fault", "untyped", "wrong_refusal"}
STALL = 45.0
# The cells whose point is a page fault: the rest of the suite fails on any Xid.
FAULTS = {"tool_guard": {31}}
JSON: TypeAlias = str | int | float | bool | None | list["JSON"] | dict[str, "JSON"]
Step: TypeAlias = dict[str, JSON]  # one JSON line a role printed (cells.py's `say`)
Cell: TypeAlias = Callable[["Context"], None]


class CellResult(TypedDict):
    cell: str
    result: str  # PASS, FAIL or N/A
    problems: list[str]
    xids: list[int]
    seconds: float
    outcomes: dict[str, JSON]
    notes: dict[str, JSON]


class Args(argparse.Namespace):
    out: Path
    cells: list[str]


class Nvml:
    """Xid events and used memory, with no kernel log. Rentals have none."""

    class Event(ctypes.Structure):
        data: int
        _fields_ = [("device", ctypes.c_void_p), ("type", ctypes.c_ulonglong), ("data", ctypes.c_ulonglong),
                    ("gi", ctypes.c_uint), ("ci", ctypes.c_uint)]

    class Memory(ctypes.Structure):
        used: int
        _fields_ = [("total", ctypes.c_ulonglong), ("free", ctypes.c_ulonglong), ("used", ctypes.c_ulonglong)]

    def __init__(self) -> None:
        self.lib = ctypes.CDLL("libnvidia-ml.so.1")
        assert self.lib.nvmlInit_v2() == 0
        self.device, self.events = ctypes.c_void_p(), ctypes.c_void_p()
        assert self.lib.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(self.device)) == 0
        assert self.lib.nvmlEventSetCreate(ctypes.byref(self.events)) == 0
        assert self.lib.nvmlDeviceRegisterEvents(self.device, ctypes.c_ulonglong(8), self.events) == 0
        self.xids: list[tuple[float, int]] = []
        threading.Thread(target=self._watch, daemon=True).start()

    def _watch(self) -> None:
        while True:
            event = self.Event()
            if self.lib.nvmlEventSetWait_v2(self.events, ctypes.byref(event), 1000) == 0:
                self.xids.append((time.monotonic(), int(event.data)))

    def used(self) -> int:
        memory = self.Memory()
        assert self.lib.nvmlDeviceGetMemoryInfo(self.device, ctypes.byref(memory)) == 0
        return memory.used


class Child:
    """One role in a fresh process. Its stdout lines are progress."""

    def __init__(self, ctx: Context, role: str, **env: str) -> None:
        self.ctx, self.role = ctx, role
        self.err = (ctx.dir / f"{role}.err").open("w")
        self.log = (ctx.dir / f"{role}.out").open("w")
        self.steps: list[Step] = []
        self.last = time.monotonic()
        self.killed = False
        self.process = subprocess.Popen(
            [sys.executable, str(HERE / "cells.py"), ctx.cell, role], stdout=subprocess.PIPE, stderr=self.err,
            text=True, env={**os.environ, **ctx.env, **env}, start_new_session=True)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.log.write(line)
            self.log.flush()
            self.last = time.monotonic()
            try:
                self.steps.append(json.loads(line))
            except ValueError:
                pass

    def alive(self) -> bool:
        return self.process.poll() is None

    def step(self, name: str, stall: float = STALL) -> Step | None:
        """The named step, or None once the role stops making progress or ends without it."""
        while True:
            for step in self.steps:
                if step.get("step") == name:
                    return step
            if not self.alive():
                time.sleep(0.2)
                if not any(step.get("step") == name for step in self.steps):
                    self.ctx.problem(f"{self.role} exited {self.process.returncode} before {name}")
                    return None
                continue
            if time.monotonic() - self.last > stall:
                self.ctx.problem(f"HANG {self.role} waiting for {name}, last step {self.last_step()}")
                self.kill()
                return None
            time.sleep(0.002)

    def last_step(self) -> str:
        return cast(str, self.steps[-1].get("step", "?")) if self.steps else "none"  # `say` names each step

    def kill(self) -> None:
        self.killed = True
        if self.alive():
            os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait()

    def finish(self, stall: float = STALL) -> Step | None:
        """Wait for it to end, watching progress, then read its verdict."""
        while self.alive():
            if time.monotonic() - self.last > stall:
                self.ctx.problem(f"HANG {self.role}, last step {self.last_step()}")
                self.kill()
                break
            time.sleep(0.02)
        self.reader.join(5)
        self.ctx.finished(self)
        return next((s for s in reversed(self.steps) if s.get("step") == "verdict"), None)


class Context:
    def __init__(self, cell: str, root: Path, nvml: Nvml) -> None:
        self.cell, self.nvml = cell, nvml
        self.dir = root / cell
        self.dir.mkdir(parents=True, exist_ok=True)
        self.problems: list[str] = []
        self.notes: dict[str, JSON] = {}
        self.children: list[Child] = []
        self.env = {"FAULT_DIR": str(self.dir)}

    def problem(self, text: str) -> None:
        self.problems.append(text)

    def child(self, role: str, **env: str) -> Child:
        child = Child(self, role, **env)
        self.children.append(child)
        return child

    def touch(self, name: str) -> None:
        (self.dir / name).touch()

    def judged(self, child: Child, stall: float = STALL) -> Step | None:
        """A role's verdict, with anything wrong recorded."""
        result = child.finish(stall)
        if result is None:
            if not child.killed:
                self.problem(f"{child.role} ended with no verdict, exit {child.process.returncode}")
            return None
        self.notes[child.role] = {k: v for k, v in result.items() if k != "step"}
        if result["outcome"] in PROBLEM_OUTCOMES:
            self.problem(f"{child.role}: {result['outcome']}")
        return result

    def finished(self, child: Child) -> None:
        code = child.process.returncode
        if not child.killed and code != 0:
            self.problem(f"{child.role} exited {code}")


def main_only(ctx: Context) -> None:
    ctx.judged(ctx.child("main"))


def killed_during(delay: float = 0.0, **env: str) -> Cell:
    """A peer keeps working while this executor dies at its `kill_me` step; a new one then joins."""

    def cell(ctx: Context) -> None:
        peer = ctx.child("survivor")
        if peer.step("ready") is None:
            return
        victim = ctx.child("main", **env)
        at = victim.step("kill_me")
        if at is None:
            ctx.touch("stop")
            ctx.judged(peer)
            return
        time.sleep(delay)
        before = len(peer.steps)
        victim.kill()
        pid = victim.process.pid
        ctx.notes["killed_at"] = at.get("at")
        # The peer must keep going: three more beats, and time for the row to retire.
        deadline = time.monotonic() + 30
        while len(peer.steps) < before + 3 and time.monotonic() < deadline and peer.alive():
            time.sleep(0.05)
        if len(peer.steps) < before + 3:
            ctx.problem(f"survivor stopped beating after the kill, last step {peer.last_step()}")
        time.sleep(5)
        ctx.touch("stop")
        result = ctx.judged(peer)
        if result and pid in cast(Sequence[int], result.get("ledger_pids", ())):  # survivor's verdict: its ledger's pids
            ctx.problem("the killed executor still has a ledger row")

    return cell


def idle_bytes() -> str:
    """What a donor holds idle: half the card, so the cell fits any card."""
    total = subprocess.check_output(["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits", "-i", "0"])
    return str((int(total) // 2) << 20)


def donor_answers(ctx: Context, **env: str) -> None:
    """The control for kill_donor_trim: with no kill the asker gets its memory from the donor."""
    idle = idle_bytes()
    donor = ctx.child("donor", FAULT_IDLE=idle)
    if donor.step("ready", stall=120) is None:
        return
    asker = ctx.child("asker", FAULT_IDLE=idle, **env)
    if asker.step("ready") is None:
        donor.kill()
        return
    ctx.touch("go")
    result = ctx.judged(asker, stall=60)
    donor.kill()
    if result and result["outcome"] != "correct":
        ctx.problem(f"a live idle donor did not cover the asker: {result['outcome']}")


def donor_answers_capped(ctx: Context) -> None:
    """Under a machine's limit too: a peer's idle bytes count against it, so it is asked."""
    donor_answers(ctx, FAULT_CAP="1")


def kill_donor(ctx: Context) -> None:
    """Kill the donor while the asker's allocation has it unmapping, seen in the driver's used bytes."""
    idle = idle_bytes()
    donor = ctx.child("donor", FAULT_IDLE=idle)
    if donor.step("ready", stall=120) is None:
        return
    asker = ctx.child("asker", FAULT_IDLE=idle)
    if asker.step("ready") is None:
        donor.kill()
        return
    before = ctx.nvml.used()
    ctx.touch("go")
    deadline = time.monotonic() + 30
    fell = None
    while time.monotonic() < deadline and donor.alive() and asker.alive():
        used = ctx.nvml.used()
        if before - used >= 1 << 30:
            fell = before - used
            break
    if fell is None:
        ctx.problem("the donor never started trimming")
    ctx.notes["killed_after_trimming_bytes"] = fell
    donor.kill()
    ctx.judged(asker, stall=60)


def foreign_grab(ctx: Context) -> None:
    worker = ctx.child("worker")
    if worker.step("ready") is None:
        return
    foreign = ctx.child("foreign")
    grabbed = foreign.step("grabbed", stall=90)
    ctx.notes["grabbed"] = grabbed and grabbed.get("grabbed")
    beats_before = len(worker.steps)
    time.sleep(12)
    during = len(worker.steps) - beats_before
    ctx.notes["worker_steps_while_held"] = during
    if during == 0 and worker.alive():
        ctx.problem("worker made no progress while the memory was held")
    ctx.touch("foreign-leave")
    ctx.judged(foreign)
    time.sleep(6)
    ctx.touch("stop")
    ctx.judged(worker)


CELLS: dict[str, Cell] = {
    "kill_capture": killed_during(),
    "kill_replay": killed_during(delay=0.5),
    "kill_fill": killed_during(delay=0.2),
    "kill_fill_gated": killed_during(),
    "kill_donor_trim": kill_donor,
    "donor_answers": donor_answers,
    "donor_answers_capped": donor_answers_capped,
    "source_partial": main_only,
    "source_prepare_error": main_only,
    "capture_oom_pool": main_only,
    "capture_oom_physical": main_only,
    "create_oom_retry": main_only,
    "create_oom_refused": main_only,
    "fit_pool_rng_late": main_only,
    "evict_fill_gated": main_only,
    "evict_fill_stress": main_only,
    "raw_graph": main_only,
    "raw_graph_prepared": main_only,
    "fork": main_only,
    "foreign_grab": foreign_grab,
    "unmap_under_kernel": main_only,
    "remap_scrambled": main_only,
    "same_skip": main_only,
    "aux_write": main_only,
    "alias_unmapped": main_only,
    "lazy_workspace": main_only,
    "no_stream_eager": main_only,
    "no_stream_capture": main_only,
    "no_stream_graph": main_only,
    "workspace_unmapped": main_only,
    "workspace_reuse": main_only,
    "tool_unmap": main_only,
    "tool_perturb": main_only,
    "guard_control": main_only,
    "tool_guard": main_only,
}


def compute_apps() -> str:
    return subprocess.run(["nvidia-smi", "--query-compute-apps=pid", "--format=csv,noheader"],
                          capture_output=True, text=True).stdout.strip()


def run_cell(cell: str, root: Path, nvml: Nvml, baseline: int) -> CellResult:
    ctx = Context(cell, root, nvml)
    began = time.monotonic()
    try:
        CELLS[cell](ctx)
    except Exception as error:  # noqa: BLE001 - a harness error is reported, never hidden
        ctx.problem(f"harness: {type(error).__name__}: {error}")
    for child in ctx.children:
        if child.alive():
            child.kill()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and (compute_apps() or nvml.used() > baseline + (16 << 20)):
        time.sleep(0.25)
    recovered_s = round(time.monotonic() - began, 1)
    if compute_apps():
        ctx.problem(f"GPU process still alive: {compute_apps()}")
    if nvml.used() > baseline + (16 << 20):
        ctx.problem(f"GPU memory not returned: {nvml.used() - baseline} bytes over baseline")
    probe = ctx.child("probe")
    outcome = ctx.judged(probe)
    if outcome is None or outcome["outcome"] != "correct":
        ctx.problem("recovery probe failed")
    time.sleep(1.5)
    xids = [xid for at, xid in nvml.xids if at >= began]
    if set(xids) - FAULTS.get(cell, set()):
        ctx.problem(f"Xid {sorted(set(xids))}")
    outcomes = {c.role: (c.steps[-1].get("outcome") if c.steps else None) for c in ctx.children}
    na = outcomes.get("main") == "n/a"
    return {"cell": cell, "result": "N/A" if na else "FAIL" if ctx.problems else "PASS", "problems": ctx.problems,
            "xids": xids, "seconds": recovered_s, "outcomes": outcomes, "notes": ctx.notes}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("cells", nargs="*")
    args = parser.parse_args(namespace=Args())
    args.out.mkdir(parents=True, exist_ok=True)
    nvml = Nvml()
    assert not compute_apps(), "the GPU is in use"
    baseline = nvml.used()
    results: list[CellResult] = []
    for cell in args.cells or CELLS:
        result = run_cell(cell, args.out, nvml, baseline)
        results.append(result)
        print(json.dumps(result), flush=True)
        (args.out / "progress").write_text(cell)
    summary = {r["cell"]: r["result"] for r in results}
    print("SUMMARY", json.dumps(summary), flush=True)
    (args.out / "summary.txt").write_text("\n".join(json.dumps(r) for r in results) + "\n")
    return 1 if "FAIL" in summary.values() else 0


if __name__ == "__main__":
    sys.exit(main())
