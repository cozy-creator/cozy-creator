#!/usr/bin/env python3
"""A run on `local` after a lock that ended one (cl-1082; coordinator 11-16: "an ordinary user who locks mid-render").

In a synthetic encrypted project on this computer's machine, through the installed CLI, nothing stopped:
1. A run on `local` is going; `cozy vault lock` ends it and locks.
2. Unlock, a run on `local` completes, lock.
3. Unlock with a 30 s maximum: a run on `local` completes. On the RC it waited for ever ("local refused to attach
   (private_storage_unavailable): the private execution marker does not match this mounted project") and held the vault open past
   its maximum.
4. The vault locks at its maximum.
On a refusal it records the view's marker against the view and the keybag: its path, generation, project and time, never its token.
The caller owns the vault before and after: it is left unlocked with no idle limit, as it was found.
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
from vault_work import Run, cli, locked_within, run_of, start, state, unlock  # noqa: E402

MAXIMUM = 30
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
    lock = cast(dict[str, object], cli(args, "lock-ending", "vault", "lock"))
    receipt["ended"] = {"run": run_of(stdout).get("number"), "lock_ended": lock.get("ended"), "locked_s": round(locked_within(args, 60), 1)}
    process.wait(timeout=120)

    # 2. Unlock, a run on `local`, lock.
    view = unlock(args, "unlock-2", 0)
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
    unlock(args, "unlock-after", 0)
    (args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(f"vault reattach: {json.dumps(receipt)}", flush=True)


if __name__ == "__main__":
    main()
