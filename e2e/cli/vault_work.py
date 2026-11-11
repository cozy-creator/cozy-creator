#!/usr/bin/env python3
"""Work in progress and an encrypted project's vault (xs-172, owner 11-11: "a render should obviously not fail").

In an unlocked synthetic vault on this computer's machine, through the installed CLI:
1. A run that outlives the idle window holds the vault open, and the idle clock starts when it ends.
2. An explicit lock mid-run ends the run first, says so, and locks.
3. A run that outlives a short maximum unlock (cl-882, owner 11-11: "runs count as activity") goes on with the vault
   unlocked, finishes and lands its file, and then the vault locks.
The caller owns the vault before and after: it is left unlocked with no idle limit, as it was found.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time
from typing import TypedDict, cast


class Vault(TypedDict, total=False):
    """`cozy vault status`: the fields this proof reads."""

    state: str
    view: str


class Changed(TypedDict, total=False):
    """A vault change's answer: the fields this proof reads."""

    vault: Vault
    ended: list[int]


class Output(TypedDict, total=False):
    """A run's output item: the fields this proof reads."""

    name: str
    status: str


class Run(TypedDict, total=False):
    """A run as the CLI answers it: the fields this proof reads."""

    number: int
    status: str
    machine_state: str
    output: list[Output]
    settled_at_ms: int

IDLE = 20
OUTLIVES = 50
MAXIMUM = 15


def cli(args: argparse.Namespace, name: str, *words: str, fails: bool = False) -> object:
    done = subprocess.run([str(args.cli), "--json", *words], cwd=args.project, capture_output=True)
    (args.out / f"{name}.json").write_bytes(done.stdout)
    (args.out / f"{name}.stderr").write_bytes(done.stderr)
    if bool(done.returncode) != fails:
        raise AssertionError(f"{name}: exit {done.returncode}: {done.stdout[-600:]!r} {done.stderr[-600:]!r}")
    try:
        answer: object = json.loads(done.stdout)
    except ValueError:
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        answer = json.loads(lines[-1]) if lines else {}
    return answer


def state(args: argparse.Namespace) -> str:
    return cast(Vault, cli(args, "status", "vault", "status"))["state"]


def unlock(args: argparse.Namespace, name: str, idle: int, maximum: int = 0) -> Path:
    unlocked = cast(Changed, cli(args, name, "vault", "unlock", "--passphrase-file", str(args.passphrase_file), "--idle", str(idle),
                                 "--max-duration", str(maximum)))
    assert unlocked["vault"]["state"] == "unlocked", f"{name}: {unlocked}"
    return Path(unlocked["vault"]["view"])


def locked_within(args: argparse.Namespace, seconds: float) -> float:
    began = time.monotonic()
    while time.monotonic() - began < seconds:
        if state(args) == "locked":
            return time.monotonic() - began
        time.sleep(0.5)
    raise AssertionError(f"the vault did not lock within {seconds} s")


def start(args: argparse.Namespace, name: str, script: Path, seconds: int, mark: str = "") -> tuple[subprocess.Popen[bytes], Path]:
    stdout = args.out / f"{name}.ndjson"
    words = [str(args.cli), "--json", "run", str(script), f"seconds:={seconds}", *([f"mark={mark}"] if mark else []), "--on", "local"]
    process = subprocess.Popen(words,
                               cwd=args.project, stdout=stdout.open("wb"), stderr=(args.out / f"{name}.stderr").open("wb"))
    return process, stdout


def run_of(stdout: Path) -> Run:
    for line in reversed(stdout.read_bytes().splitlines()):
        try:
            event: object = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and isinstance(event.get("run"), dict):
            return cast(Run, event["run"])
    return {}


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
    view = unlock(args, "unlock-idle", IDLE)
    folder = view / "vault-work-proof"
    folder.mkdir(exist_ok=True)
    script = folder / "sleeper.py"
    script.write_text('''# /// script
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
def vault_work_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    target = out.temporary_file(".txt")
    target.write_text(f"{payload.mark} slept {payload.seconds}")
    return Answer(payload.seconds, out.save_file(target, media_type="text/plain"))
''')
    # Its environment installed first, so the timed run below is the sleep, not an install.
    cli(args, "warm", "run", str(script), "seconds:=0", "--on", "local")

    # 1. A run longer than the idle window: the vault stays open throughout, and locks about IDLE after the run ends.
    process, stdout = start(args, "outlives", script, OUTLIVES)
    began, seen = time.monotonic(), set[str]()
    while process.poll() is None:
        seen.add(state(args))
        time.sleep(2)
    took = time.monotonic() - began
    assert process.returncode == 0, f"the run outliving the idle window failed: {run_of(stdout)}"
    assert took > IDLE + 10, f"the run took {took:.0f} s, not past the {IDLE} s idle window"
    assert seen == {"unlocked"}, f"the vault was {sorted(seen)} while the run went on"
    assert state(args) == "unlocked", "the vault locked as the run ended, before its idle window"
    after = locked_within(args, IDLE + 30)
    assert after >= IDLE - 5, f"the vault locked {after:.0f} s after the run ended, before its {IDLE} s idle window"
    receipt["outlived"] = {"run": run_of(stdout).get("number"), "run_s": round(took), "idle_s": IDLE, "locked_after_end_s": round(after)}

    # 2. An explicit lock mid-run ends the run first, says so, and locks.
    # Each unlock mounts the view afresh, at a new path.
    script = unlock(args, "unlock-for-lock", 0) / "vault-work-proof" / "sleeper.py"
    process, stdout = start(args, "ended", script, 900)
    while (run := run_of(stdout)).get("machine_state") != "running":
        assert process.poll() is None, f"the run to be ended stopped first: {run}"
        time.sleep(1)
    number = run["number"]
    lock = cast(Changed, cli(args, "lock-mid-run", "vault", "lock"))
    assert number in lock.get("ended", []), f"the lock did not say it ended run {number}: {lock}"
    assert lock["vault"]["state"] in ("locking", "locked"), f"the lock left the vault {lock['vault']['state']}"
    lock_s = locked_within(args, 120)
    process.wait(timeout=120)
    unlock(args, "unlock-after", 0)
    # `cozy run watch` exits 1 for a canceled run.
    watched = cast(Run, cli(args, "ended-run", "run", "watch", str(number), fails=True))
    assert watched.get("status") == "canceled", f"run {number} is {watched.get('status')} after the lock ended it"
    receipt["lock_ended"] = {"run": number, "ended": lock["ended"], "state": lock["vault"]["state"], "locked_s": round(lock_s, 1), "run_status": watched["status"]}

    # 3. A run longer than a short maximum: the vault stays unlocked while it goes on, it finishes and lands, then the vault locks.
    cli(args, "lock-before-maximum", "vault", "lock")
    deadline_ms = time.time() * 1000 + MAXIMUM * 1000
    script = unlock(args, "unlock-maximum", 0, MAXIMUM) / "vault-work-proof" / "sleeper.py"
    # Not `token`: TensorD refuses a field of that name as a credential.
    mark = f"cl882-{time.time_ns()}"
    process, stdout = start(args, "past-maximum", script, OUTLIVES, mark)
    # Each state as read by then (wall ms, Calcifer's clock too): the vault may lock as soon as the run settles.
    samples: list[tuple[float, str]] = []
    while process.poll() is None:
        read = state(args)
        samples.append((time.time() * 1000, read))
        time.sleep(2)
    run = run_of(stdout)
    assert process.returncode == 0, f"the run outliving the maximum failed: {run}"
    assert run.get("status") == "completed", f"the run outliving the maximum is {run.get('status')}"
    settled_ms = run["settled_at_ms"]
    running = [read for at, read in samples if at < settled_ms]
    past = [read for at, read in samples if deadline_ms + 1000 < at < settled_ms]
    assert set(running) == {"unlocked"}, f"the vault was {sorted(set(running))} while the run went on"
    assert len(past) >= 5, f"only {len(past)} reads between the {MAXIMUM} s maximum and the run's end"
    landed = [item for item in run.get("output", []) if item.get("status") == "completed"]
    assert landed, f"run {run.get('number')} landed no file: {run.get('output')}"
    max_lock_s = locked_within(args, 30)
    view = unlock(args, "unlock-after-maximum", 0)
    kept = [path for path in (view / ".runs").rglob("*") if path.is_file() and mark.encode() in path.read_bytes()]
    assert kept, f"run {run.get('number')}'s file is not in the project after the vault locked"
    receipt["outlived_maximum"] = {"run": run.get("number"), "status": run.get("status"), "maximum_s": MAXIMUM,
                                   "unlocked_past_maximum_s": round((settled_ms - deadline_ms) / 1000), "unlocked_reads_past_maximum": len(past),
                                   "locked_after_end_s": round(max_lock_s, 1), "landed": str(kept[0].relative_to(view))}
    receipt["synthetic_only"] = True
    (args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(f"vault work: {json.dumps(receipt)}", flush=True)


if __name__ == "__main__":
    main()
