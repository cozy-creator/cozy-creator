#!/usr/bin/env python3
"""A run on `local` after a lock that ended one (cl-1082; coordinator 11-16: "an ordinary user who locks mid-render").

In a synthetic encrypted project on this computer's machine, through the installed CLI, nothing stopped:
1. A run on `local` is going; `cozy vault lock` ends it and locks.
2. Unlock, a run on `local` completes, lock.
3. Unlock with a 30 s maximum: a run on `local` completes. On the RC it waited for ever ("local refused to attach
   (private_storage_unavailable): the private execution marker does not match this mounted project") and held the vault open past
   its maximum.
4. The vault locks at its maximum.
5. Unlock with a 30 s maximum: a run on `local` that has started stalls with its machine (SIGSTOP for 45 s, then SIGCONT), and
   re-attaches and completes; the vault locks once it has. In cl-1064's cell such a run was refused at its re-attach.
On a refusal it records the view's marker against the view and the keybag: its path, generation, project and time, never its token.
The caller owns the vault before and after: it is left unlocked with no idle limit, as it was found.
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
from vault_work import Run, cli, locked_within, run_of, start, state, unlock  # noqa: E402

MAXIMUM = 30
STALL = 45
SLEEPER = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import time
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
class Request(msgspec.Struct, forbid_unknown_fields=True):
    seconds: int
    mark: str = ""
class Answer(msgspec.Struct):
    slept: int
    file: File
@app.entrypoint(accelerator=False)
def vault_reattach_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    target = out.temporary_file(".txt")
    target.write_text(f"{payload.mark} slept {payload.seconds}")
    return Answer(payload.seconds, out.save_file(target, media_type="text/plain"))
'''


def marker(view: Path, project: Path) -> dict[str, object]:
    """The view's execution marker against the view and the keybag, its token left out."""
    facts: dict[str, object] = {"view": str(view), "at": round(time.time(), 1)}
    path = view / ".vault-local-execution.json"
    try:
        found = json.loads(path.read_bytes())
        storage = found.get("storage", {})
        facts["marker"] = {"view_root": storage.get("view_root"), "generation": storage.get("generation"),
                           "project_id": storage.get("project_id"), "mtime": round(path.stat().st_mtime, 1)}
    except (OSError, ValueError, AttributeError) as error:
        facts["marker"] = f"unreadable: {type(error).__name__}"
    try:
        keybag = json.loads((project / "project.vault.keys.json").read_bytes())
        facts["keybag"] = {key: value for key, value in walk(keybag) if key in ("generation", "project_id")}
    except (OSError, ValueError) as error:
        facts["keybag"] = f"unreadable: {type(error).__name__}"
    return facts


def walk(value: object) -> list[tuple[str, object]]:
    if isinstance(value, dict):
        return [pair for key, inner in value.items() for pair in ([(key, inner)] if not isinstance(inner, (dict, list)) else walk(inner))]
    if isinstance(value, list):
        return [pair for inner in value for pair in walk(inner)]
    return []


def machine_pid() -> int:
    """The local machine's process: what holds its root's lock, `cozy machine serve`."""
    holder = Path.home() / ".cozy/machine/var/lib/cozy/machine/agent.lock.holder"
    pid = int(json.loads(holder.read_text())["pid"])
    command = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
    assert "serve" in command, f"the local machine {pid} is {command!r}"
    return pid


def stall(seconds: float) -> dict[str, object]:
    """The local machine stopped `seconds` and then continued, as a machine under load or swap stalls."""
    pid = machine_pid()
    os.kill(pid, signal.SIGSTOP)
    try:
        state = ""
        for _ in range(50):
            state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
            if state == "T":
                break
            time.sleep(0.1)
        time.sleep(seconds)
    finally:
        os.kill(pid, signal.SIGCONT)
    return {"pid": pid, "state": state, "seconds": seconds}


def started(stdout: Path) -> bool:
    return bool(cast(dict[str, object], run_of(stdout)).get("started_at_ms"))


def ran(args: argparse.Namespace, name: str, script: Path, seconds: int, within: float) -> tuple[Run, str]:
    """A run on `local` to its end within `within` s: its last state, and the last reason it waited."""
    process, stdout = start(args, name, script, seconds)
    try:
        process.wait(timeout=within)
    except subprocess.TimeoutExpired:
        process.kill()
    waited = ""
    for line in stdout.read_bytes().splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and event.get("type") == "run.waiting" and event.get("reason"):
            waited = str(event["reason"])
    return run_of(stdout), waited


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    receipt: dict[str, object] = {}

    if state(args) != "locked":
        cli(args, "lock-before", "vault", "lock")
    view = unlock(args, "unlock-1", 0)
    script = view / "vault-reattach" / "sleeper.py"
    script.parent.mkdir(exist_ok=True)
    script.write_text(SLEEPER)
    cli(args, "warm", "run", str(script), "seconds:=0", "--on", "local")
    receipt["unlock_1"] = marker(view, args.project)

    # 1. A lock while a run on `local` is going: it ends the run, then locks.
    process, stdout = start(args, "ended", script, 120)
    began = time.monotonic()
    while time.monotonic() - began < 120 and not started(stdout):
        time.sleep(0.5)
    assert started(stdout), f"the run to end never started: {run_of(stdout)}"
    parent = machine_pid()
    os.kill(parent, signal.SIGSTOP)
    try:
        lock = cast(dict[str, object], cli(args, "lock-ending", "vault", "lock"))
        receipt["ended"] = {"run": run_of(stdout).get("number"), "lock_ended": lock.get("ended"), "locked_s": round(locked_within(args, 60), 1),
                            "parent_stopped": parent}
        # 2. Unlock (the parent still stopped, as lock_hung's was), a run on `local`, lock.
        view = unlock(args, "unlock-2", 0)
    finally:
        os.kill(parent, signal.SIGCONT)
    process.wait(timeout=120)

    script = view / "vault-reattach" / "sleeper.py"
    receipt["unlock_2"] = marker(view, args.project)
    run, waited = ran(args, "between", script, 0, 300)
    assert run.get("status") == "completed", f"the run after the ending lock: {run.get('status')}, waiting {waited!r}: {receipt}"
    cli(args, "lock-2", "vault", "lock")
    locked_within(args, 60)

    # 3. Unlock with a maximum: a run on `local` attaches and completes.
    view = unlock(args, "unlock-3", 0, MAXIMUM)
    script = view / "vault-reattach" / "sleeper.py"
    unlocked = time.monotonic()
    receipt["unlock_3"] = marker(view, args.project)
    run, waited = ran(args, "after", script, 0, 120)
    receipt["after"] = {"run": run.get("number"), "status": run.get("status"), "waited": waited, "marker": marker(view, args.project)}
    print(f"vault reattach: {json.dumps(receipt)}", flush=True)
    if run.get("status") != "completed":
        subprocess.run([str(args.cli), "--json", "run", "cancel", str(run.get("number"))], cwd=args.project, capture_output=True)
    assert run.get("status") == "completed", f"the run after lock, unlock, run, lock, unlock: {receipt['after']}"

    # 4. The maximum locks it.
    receipt["locked_at_maximum_s"] = round(time.monotonic() - unlocked + locked_within(args, MAXIMUM + 60 - (time.monotonic() - unlocked)), 1)

    # 5. A run that has started stalls with its machine, re-attaches and completes; then the vault locks.
    view = unlock(args, "unlock-5", 0, MAXIMUM)
    script = view / "vault-reattach" / "sleeper.py"
    receipt["unlock_5"] = marker(view, args.project)
    process, stdout = start(args, "stalled", script, 90)
    receipt["stall"] = stall(STALL)
    seen: list[dict[str, object]] = []
    began = time.monotonic()
    while process.poll() is None and time.monotonic() - began < 240:
        found = marker(view, args.project).get("marker")
        if not seen or found != seen[-1].get("marker"):
            seen.append({"t": round(time.monotonic() - began, 1), "marker": found})
        time.sleep(2)
    if process.poll() is None:
        process.kill()
    receipt["marker_changes"] = seen
    run, waited = run_of(stdout), ""
    for line in stdout.read_bytes().splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and event.get("type") == "run.waiting" and event.get("reason"):
            waited = str(event["reason"])
    receipt["stalled"] = {"run": run.get("number"), "status": run.get("status"), "waited": waited, "marker": marker(view, args.project)}
    print(f"vault reattach: {json.dumps(receipt)}", flush=True)
    if run.get("status") != "completed":
        subprocess.run([str(args.cli), "--json", "run", "cancel", str(run.get("number"))], cwd=args.project, capture_output=True)
    assert run.get("status") == "completed", f"the run that stalled with its machine: {receipt['stalled']}"
    receipt["locked_after_stalled_s"] = round(locked_within(args, 90), 1)
    unlock(args, "unlock-after", 0)
    (args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(f"vault reattach: {json.dumps(receipt)}", flush=True)


if __name__ == "__main__":
    main()
