#!/usr/bin/env python3
"""A run its machine hasn't begun never holds a vault past its maximum (cl-1082; coordinator 11-16: "a vault past its maximum is the
privacy promise").

A fresh synthetic encrypted project, unlocked with a 30 s maximum. A published tool's run is asked of a rental on its way (the
isolated Hub's, which never serves on this runner): its machine never begins it. The vault locks at its maximum all the same, and
the run fails `vault_locked`, never begun. On the RC the run held the vault open for as long as its machine was on its way.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent))
from vault_work import cli, run_of, state, unlock  # noqa: E402

MAXIMUM = 30
BOUND = 45
# Long enough never to lock it first: Calcifer's diagnostics then say what holds its idle clock, every 10 s.
IDLE = 600


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--machine", required=True, help="a rental on its way, which never serves here")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    if state(args) != "locked":
        cli(args, "lock-before", "vault", "lock")
    unlock(args, "unlock", IDLE, MAXIMUM)
    unlocked = time.monotonic()
    stdout = args.out / "unstarted.ndjson"
    process = subprocess.Popen([str(args.cli), "--json", "run", "minimax-h3-tools/echo", "text=vault unstarted", "--on", args.machine],
                               cwd=args.project, stdout=stdout.open("wb"), stderr=(args.out / "unstarted.stderr").open("wb"))
    locked_s: float | None = None
    while time.monotonic() - unlocked < MAXIMUM + BOUND:
        if state(args) == "locked":
            locked_s = round(time.monotonic() - unlocked, 1)
            break
        time.sleep(1)
    try:
        process.wait(timeout=30)
    except subprocess.TimeoutExpired:
        process.kill()
    run = cast(dict[str, object], run_of(stdout))
    reason = run.get("reason")
    facts = {"maximum_s": MAXIMUM, "locked_s": locked_s, "run": run.get("number"), "machine": run.get("machine"), "status": run.get("status"),
             "reason": reason.get("code") if isinstance(reason, dict) else None, "started_at_ms": run.get("started_at_ms")}
    (args.out / "receipt.json").write_text(json.dumps(facts, indent=2))
    print(f"vault unstarted: {json.dumps(facts)}", flush=True)
    if run.get("status") not in ("completed", "failed", "canceled") and run.get("number"):
        subprocess.run([str(args.cli), "--json", "run", "cancel", str(run.get("number"))], cwd=args.project, capture_output=True)
    assert run.get("number"), f"no run was asked of {args.machine}: {stdout.read_text()[-400:]!r}"
    assert locked_s is not None, f"the vault was open {MAXIMUM + BOUND} s past a {MAXIMUM} s maximum, held by a run its machine hadn't begun: {facts}"
    assert not facts["started_at_ms"], f"the run began after all: {facts}"
    assert facts["status"] == "failed" and facts["reason"] == "vault_locked", f"the run its machine hadn't begun, at the maximum: {facts}"


if __name__ == "__main__":
    main()
