#!/usr/bin/env python3
"""Work in progress and an encrypted project's vault (xs-172, owner 11-11: "a render should obviously not fail").

In an unlocked synthetic vault on this computer's machine, through the installed CLI:
1. A run that outlives the idle window holds the vault open, and the idle clock starts when it ends.
2. An explicit lock mid-run ends the run first, says so, and locks.
3. A run that outlives a short maximum unlock (cl-882, owner 11-11: "runs count as activity") goes on with the vault
   unlocked, finishes and lands its file, and then the vault locks. Its file is video-sized (cl-1060): saved into the project
   after its call ends, past the maximum, which the vault waits for; a lock could cut the save off at the run's end.
   Then Calcifer's own logs name neither the project's code nor its runs.
4. An explicit lock as a run's output starts coming in goes through at once; the next unlock saves the file whole.
The caller owns the vault before and after: it is left unlocked with no idle limit, as it was found.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import re
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
    unsaved: list[str]


class Output(TypedDict, total=False):
    """A run's output item: the fields this proof reads."""

    name: str
    status: str
    sha256: str
    path: str


class Run(TypedDict, total=False):
    """A run as the CLI answers it: the fields this proof reads."""

    name: str
    number: int
    status: str
    machine_state: str
    output: list[Output]
    settled_at_ms: int
    progress: dict[str, object]

IDLE = 20
OUTLIVES = 50
MAXIMUM = 15
# The maximum's run's file, in MB: a save into the project that takes a while after its call ends (cl-1060).
VIDEO_MB = 128
# Step 4's file, in MB: big enough that its save into the project takes seconds, so a lock lands in it (cl-1060).
SAVE_MB = 2048


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


def start(args: argparse.Namespace, name: str, script: Path, seconds: int, mark: str = "", size: int = 0) -> tuple[subprocess.Popen[bytes], Path]:
    stdout = args.out / f"{name}.ndjson"
    words = [str(args.cli), "--json", "run", str(script), f"seconds:={seconds}", *([f"mark={mark}"] if mark else []),
             *([f"size:={size}"] if size else []), "--on", "local"]
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


def exporting(stdout: Path, process: subprocess.Popen[bytes]) -> bool:
    """Until the run's output starts coming into the project (its call ended, its row settled), or it ends."""
    while process.poll() is None:
        for line in stdout.read_bytes().splitlines():
            if b'"output_item.added"' in line:
                return True
        time.sleep(0.02)
    return False


def settled(args: argparse.Namespace, number: object, seconds: float) -> tuple[str, Run]:
    """Run `number`'s status once it settles, or as it is after `seconds`, and its record."""
    began, status, record = time.monotonic(), "", cast(Run, {})
    while time.monotonic() - began < seconds:
        done = subprocess.run([str(args.cli), "--json", "run", "watch", str(number), "--once"], cwd=args.project, capture_output=True)
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        try:
            watched: object = json.loads(lines[-1]) if lines else {}
        except ValueError:
            watched = {}
        if isinstance(watched, dict) and isinstance(watched.get("run"), dict):
            watched = watched["run"]
        status = str(watched.get("status", "")) if isinstance(watched, dict) else ""
        record = cast(Run, watched) if isinstance(watched, dict) else record
        if status in ("completed", "failed", "canceled"):
            break
        time.sleep(2)
    return status, record


def items(args: argparse.Namespace, number: object) -> list[dict[str, object]]:
    """Run `number`'s outputs as `cozy run show --json` says them: status, landed path or not, export error."""
    done = subprocess.run([str(args.cli), "--json", "run", "show", str(number)], cwd=args.project, capture_output=True)
    lines = [line for line in done.stdout.splitlines() if line.strip()]
    try:
        shown: object = json.loads(lines[-1]) if lines else {}
    except ValueError:
        return []
    if isinstance(shown, dict) and isinstance(shown.get("run"), dict):
        shown = shown["run"]
    output = shown.get("output", []) if isinstance(shown, dict) else []
    return [{"name": item.get("name"), "status": item.get("status"), "landed": bool(item.get("path")), "error": item.get("export_error")}
            for item in output if isinstance(item, dict)] if isinstance(output, list) else []


def whole(args: argparse.Namespace, number: object, mark: str, name: str) -> tuple[int, str]:
    """Run `number`'s file as a person fetches it from the project: its size and digest, the mark at its start, read as a stream;
    (0, "") when it doesn't come back. The copy isn't kept."""
    fetched = args.out / name
    subprocess.run([str(args.cli), "run", "watch", str(number), "--once", "-o", str(fetched)], cwd=args.project, capture_output=True)
    size, digest = 0, ""
    for path in fetched.rglob("*"):
        if not path.is_file():
            continue
        with path.open("rb") as file:
            head = file.read(200)
            # The run's file, not its small note, which starts with the mark too.
            if mark.encode() in head and path.stat().st_size > size:
                hasher = hashlib.sha256(head)
                for chunk in iter(lambda: file.read(1 << 20), b""):
                    hasher.update(chunk)
                size, digest = path.stat().st_size, "sha256:" + hasher.hexdigest()
        path.unlink()
    return size, digest

def after_failure(args: argparse.Namespace, number: object, mark: str) -> str:
    """What a save cut off by the lock left (cl-1060): the vault locked, unlocked again, and the run watched until it settles."""
    try:
        locked = f"locked {locked_within(args, 60):.0f} s after"
    except AssertionError:
        locked = "not locked within 60 s"
    try:
        unlock(args, "unlock-after-failure", 0)
    except AssertionError as error:
        return f"{locked}; it did not unlock again: {error}"
    began, status, stage = time.monotonic(), "", ""
    while time.monotonic() - began < 120:
        done = subprocess.run([str(args.cli), "--json", "run", "watch", str(number), "--once"], cwd=args.project, capture_output=True)
        (args.out / "after-failure-watch.json").write_bytes(done.stdout)
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        try:
            watched: object = json.loads(lines[-1]) if lines else {}
        except ValueError:
            watched = {}
        if isinstance(watched, dict) and isinstance(watched.get("run"), dict):
            watched = watched["run"]
        if isinstance(watched, dict):
            status = str(watched.get("status", ""))
            progress = watched.get("progress")
            stage = str(progress.get("stage", "")) if isinstance(progress, dict) else ""
        if status in ("completed", "failed", "canceled"):
            break
        time.sleep(2)
    fetched = args.out / "after-failure.out"
    subprocess.run([str(args.cli), "run", "watch", str(number), "--once", "-o", str(fetched)], cwd=args.project, capture_output=True)
    kept = [path for path in fetched.rglob("*") if path.is_file() and mark.encode() in path.read_bytes()[:200]]
    for path in fetched.rglob("*"):
        if path.is_file():
            path.unlink()
    return (f"{locked}; at the next unlock run {number} was {status or 'unread'} ({stage or 'no stage'}) after "
            f"{time.monotonic() - began:.0f} s, and its file {'came back from the project' if kept else 'did not come back'}")


def traces(since: datetime, names: list[str]) -> list[str]:
    """Lines of Calcifer's own logs, written since `since`, that name the encrypted project's runs or its code (cl-1060): none
    may, as nothing of a locked project but its folder stays outside it."""
    code = names[0].split("-")[0]
    markers = [re.compile(rf"\b{re.escape(name)}\b") for name in names] + [re.compile(rf"\b{re.escape(code)}\b")]
    stamp = since.strftime("%Y-%m-%dT%H:%M:%S")
    logs = {path.name: [line for line in path.read_text(errors="replace").splitlines() if line[:19] >= stamp]
            for path in (Path.home() / ".cozy/calcifer").glob("*.log")}
    journal = subprocess.run(["journalctl", "--user", "-u", "cozy-calcifer.service", "--no-pager", "-o", "cat",
                              "--since", f"@{int(since.timestamp())}"], capture_output=True, text=True)
    if journal.returncode == 0:
        logs["journal"] = journal.stdout.splitlines()
    return [f"{name}: {line[:240]}" for name, lines in logs.items() for line in lines if any(marker.search(line) for marker in markers)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    receipt: dict[str, object] = {}
    since, names = datetime.now(timezone.utc), list[str]()

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
import os
import time
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
class Request(msgspec.Struct, forbid_unknown_fields=True):
    seconds: int
    mark: str = ""
    size: int = 0
class Answer(msgspec.Struct):
    slept: int
    file: File
    note: File
@app.entrypoint(accelerator=False)
def vault_work_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    target = out.temporary_file(".txt")
    with open(target, "wb") as file:
        file.write(f"{payload.mark} slept {payload.seconds}\\n".encode())
        for _ in range(payload.size):
            file.write(os.urandom(1 << 20))
    note = out.temporary_file(".txt")
    note.write_text(f"{payload.mark} note")
    return Answer(payload.seconds, out.save_file(target, media_type="text/plain"), out.save_file(note, media_type="text/plain"))
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
    process, stdout = start(args, "past-maximum", script, OUTLIVES, mark, VIDEO_MB)
    # Each state as read by then (wall ms, Calcifer's clock too): the vault may lock as soon as the run settles.
    samples: list[tuple[float, str]] = []
    while process.poll() is None:
        read = state(args)
        samples.append((time.time() * 1000, read))
        time.sleep(2)
    run = run_of(stdout)
    if process.returncode != 0 or run.get("status") != "completed":
        # Whether its output is owed or lost: the next unlock finishes the save, or not.
        print(f"vault work: after the maximum's failure: {after_failure(args, run.get('number'), mark)}", flush=True)
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
    unlock(args, "unlock-after-maximum", 0)
    # Its file comes back from the project as a person fetches it, the vault locked and unlocked since.
    fetched = args.out / "past-maximum.out"
    done = subprocess.run([str(args.cli), "run", "watch", str(run.get("number")), "--once", "-o", str(fetched)], cwd=args.project, capture_output=True)
    (args.out / "past-maximum.fetch.stderr").write_bytes(done.stderr)
    kept = [path for path in fetched.rglob("*") if path.is_file() and mark.encode() in path.read_bytes()[:200]]
    assert kept, f"run {run.get('number')}'s file did not come back from the project: exit {done.returncode} {done.stderr[-600:]!r}"
    sizes = [path.stat().st_size for path in kept]
    for path in fetched.rglob("*"):
        if path.is_file():
            path.unlink()  # 128 MB: not an artifact
    assert max(sizes) >= VIDEO_MB << 20, f"run {run.get('number')}'s file came back {max(sizes)} bytes, not {VIDEO_MB} MB"
    receipt["outlived_maximum"] = {"run": run.get("number"), "status": run.get("status"), "maximum_s": MAXIMUM,
                                   "unlocked_past_maximum_s": round((settled_ms - deadline_ms) / 1000), "unlocked_reads_past_maximum": len(past),
                                   "locked_after_end_s": round(max_lock_s, 1), "fetched": kept[0].name, "file_mb": VIDEO_MB}

    # No trace of it outside it: Calcifer's own logs name neither its code nor its runs, after a deadline lock that waited on a
    # save (step 3), a lock that ended a run (step 2), and runs that ran slow.
    names += [name for out in ("outlives", "ended", "past-maximum") if (name := run_of(args.out / f"{out}.ndjson").get("name"))]
    if names:
        found = traces(since, sorted(set(names)))
        assert not found, f"Calcifer's own logs name the encrypted project's runs or its code: {found[:5]}"
    receipt["no_traces"] = {"runs": sorted(set(names)), "logs_scanned": True}

    # 4. An explicit lock lands in a save by its size: a 2 GB file whose save into the project takes seconds, locked as it starts
    #    coming (cl-1060). What happens is recorded and printed whatever it is (the lock's time and exit, views left, half-written
    #    files, whether the next unlock saves the file whole); then the fix's behaviour is asserted.
    script = unlock(args, "unlock-for-save-lock", 0) / "vault-work-proof" / "sleeper.py"
    mark = f"cl1060-lock-{time.time_ns()}"
    process, stdout = start(args, "save-lock", script, 2, mark, SAVE_MB)
    assert exporting(stdout, process), f"the run's output never started coming: {run_of(stdout)}"
    began = time.monotonic()
    asked = subprocess.run([str(args.cli), "--json", "vault", "lock"], cwd=args.project, capture_output=True)
    lock_s = round(time.monotonic() - began, 1)
    (args.out / "lock-while-saving.json").write_bytes(asked.stdout + b"\n" + asked.stderr)
    lines = [line for line in asked.stdout.splitlines() if line.strip()]
    try:
        answer: object = json.loads(lines[-1]) if lines else {}
    except ValueError:
        answer = {}
    answer = answer if isinstance(answer, dict) else {}
    error = answer.get("error")
    try:
        locked_s: float | None = round(locked_within(args, 60) + lock_s, 1)
    except AssertionError:
        locked_s = None
    left = [line for line in Path("/proc/self/mountinfo").read_text().splitlines() if f"{args.project}/.vault-runtime/" in line]
    done_before = any(b'"output_item.done"' in line for line in stdout.read_bytes().splitlines())
    process.wait(timeout=600)
    saved = run_of(stdout).get("number")
    resumed = time.monotonic()
    view = unlock(args, "unlock-after-save-lock", 0)
    print(f"vault work: lock while saving: items at the unlock: {items(args, saved)}", flush=True)
    status, record = settled(args, saved, 600)
    save_s = round(time.monotonic() - resumed, 1)
    print(f"vault work: lock while saving: items after the save: {items(args, saved)}", flush=True)
    landed = [item for item in record.get("output", []) if item.get("path")]
    partial = [str(path) for path in view.rglob("*") if path.name.endswith((".part", ".part.held", ".manifest.part"))]
    size, digest = whole(args, saved, mark, "save-lock.out")
    made = [item.get("sha256") for item in record.get("output", [])]
    observed = {"run": saved, "file_mb": SAVE_MB, "lock_exit": asked.returncode,
                "lock_error": error.get("code") if isinstance(error, dict) else None, "unsaved": answer.get("unsaved", []),
                "lock_answered_s": lock_s, "locked_s": locked_s, "views_left": len(left), "saved_before_lock": done_before,
                "next_unlock": status, "save_after_unlock_s": save_s, "half_written": partial, "file_bytes": size,
                "digest_is_runs": digest in made, "items_landed": len(landed), "items": len(record.get("output", []))}
    print(f"vault work: lock while saving: {json.dumps(observed)}", flush=True)
    receipt["lock_while_saving"] = observed
    # Its large files aren't artifacts.
    for path in view.rglob("*"):
        if path.is_file() and path.stat().st_size > 32 << 20:
            path.unlink()
    assert asked.returncode == 0 and locked_s is not None and locked_s < 15, f"a lock during a save: {observed}"
    assert not left, f"the lock left a view mounted: {observed}"
    assert status == "completed" and not partial, f"after a lock stopped run {saved}'s save, the next unlock: {observed}"
    assert size >= SAVE_MB << 20 and digest in made, f"run {saved}'s file after the next unlock: {observed}"
    assert len(landed) == len(record.get("output", [])) == 2, f"run {saved}'s outputs after the next unlock, the one behind the big one too: {observed}"
    cli(args, "lock-after-save", "vault", "lock")

    receipt["synthetic_only"] = True
    (args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(f"vault work: {json.dumps(receipt)}", flush=True)


if __name__ == "__main__":
    main()
