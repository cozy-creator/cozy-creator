#!/usr/bin/env python3
"""A run its machine hasn't begun never holds a vault past its maximum (cl-1082; coordinator 11-16: "a vault past its maximum is the
privacy promise").

A fresh synthetic encrypted project (its machine holds nothing of it, so a lock needs no answer from it), unlocked with a 30 s
maximum. The local machine is stopped (SIGSTOP) and a run is asked of it: its machine never begins it. The vault locks at its
maximum all the same, while the machine is still stopped, and the run fails `vault_locked`, never begun. On the RC the run held the
vault open until its machine came back and ran it. The machine always goes on at the end.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent))
from vault_work import cli, run_of, start, state, unlock  # noqa: E402

MAXIMUM = 30
BOUND = 30
SLEEPER = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import time
import msgspec
from cozy_runtime.author import App, Outputs
app = App()
class Request(msgspec.Struct, forbid_unknown_fields=True):
    seconds: int
class Answer(msgspec.Struct):
    slept: int
@app.entrypoint(accelerator=False)
def vault_unstarted_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    return Answer(payload.seconds)
'''


def machine_pid() -> int:
    """The local machine's process: what holds its root's lock, `cozy machine serve`."""
    holder = Path.home() / ".cozy/machine/var/lib/cozy/machine/agent.lock.holder"
    pid = int(json.loads(holder.read_text())["pid"])
    command = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
    assert "serve" in command, f"the local machine {pid} is {command!r}"
    return pid


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    if state(args) != "locked":
        cli(args, "lock-before", "vault", "lock")
    view = unlock(args, "unlock", 0, MAXIMUM)
    unlocked = time.monotonic()
    script = view / "vault-unstarted" / "sleeper.py"
    script.parent.mkdir(exist_ok=True)
    script.write_text(SLEEPER)
    assert not (view / ".vault-machine").exists(), "the machine already holds some of this fresh project"
    pid = machine_pid()
    os.kill(pid, signal.SIGSTOP)
    locked_s: float | None = None
    try:
        process, stdout = start(args, "unstarted", script, 120)
        while time.monotonic() - unlocked < MAXIMUM + BOUND:
            if state(args) == "locked":
                locked_s = round(time.monotonic() - unlocked, 1)
                break
            time.sleep(1)
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
    finally:
        os.kill(pid, signal.SIGCONT)
    run = cast(dict[str, object], run_of(stdout))
    reason = run.get("reason")
    facts = {"maximum_s": MAXIMUM, "locked_s": locked_s, "run": run.get("number"), "status": run.get("status"),
             "reason": reason.get("code") if isinstance(reason, dict) else None, "started_at_ms": run.get("started_at_ms"),
             "machine_stopped": pid}
    (args.out / "receipt.json").write_text(json.dumps(facts, indent=2))
    print(f"vault unstarted: {json.dumps(facts)}", flush=True)
    assert locked_s is not None, f"the vault was open {MAXIMUM + BOUND} s past a {MAXIMUM} s maximum, held by a run its machine hadn't begun: {facts}"
    assert not facts["started_at_ms"], f"the run began after all: {facts}"
    assert facts["status"] == "failed" and facts["reason"] == "vault_locked", f"the run its machine hadn't begun, at the maximum: {facts}"


if __name__ == "__main__":
    main()
