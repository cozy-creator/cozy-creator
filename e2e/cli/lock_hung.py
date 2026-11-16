#!/usr/bin/env python3
"""lock_hung (cl-1064): an encrypted project locks while its local machine doesn't answer. Paul: "lock must succeed when asked".

A run sleeps in an encrypted project, then writes a file of its mark; the local machine is stopped (SIGSTOP), as a hung GPU machine
is. `cozy vault lock` locks within its bound and says local's clean-up is owed; the view is gone; and nothing of the project is
anywhere outside its folder but the machine's own journal, which only the stopped machine writes: the one exception, named. The
project is unlocked again before the machine answers. Then the machine goes on (SIGCONT): what it held of the project's last view is
cancelled and retired before anything else; the run stays canceled, its file never appears, the journal has nothing of that view,
and the project runs again. The same with the maximum's deadline in place of the lock. The machine always goes on at the end."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import sqlite3
import subprocess
import sys
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent / "containment"))
from scan import Scanner, places  # noqa: E402

SLEEPS = 45
MAXIMUM = 30
# The lock's bound: three unanswered asks of local (a second apart, two seconds each) and the lock itself.
BOUND = 60
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
    mark: str
class Answer(msgspec.Struct):
    file: File
@app.entrypoint(accelerator=False)
def lock_hung_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    target = out.temporary_file(".txt")
    with open(target, "w") as file:
        file.write(payload.mark)
    return Answer(out.save_file(target, media_type="text/plain"))
'''


class Cell:
    def __init__(self, args: argparse.Namespace) -> None:
        self.cli: Path = args.cli
        self.out: Path = args.out
        self.project: Path = args.project
        self.passphrase: Path = args.passphrase_file

    def cozy(self, name: str, *words: str, fails: bool = False, timeout: float = 300) -> dict[str, object]:
        done = subprocess.run([str(self.cli), "--json", *words], cwd=self.project, capture_output=True, timeout=timeout)
        (self.out / f"{name}.json").write_bytes(done.stdout)
        (self.out / f"{name}.stderr").write_bytes(done.stderr)
        assert bool(done.returncode) == fails, f"{name}: exit {done.returncode}: {done.stdout[-500:]!r} {done.stderr[-500:]!r}"
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        try:
            answer: object = json.loads(lines[-1]) if lines else {}
        except ValueError:
            answer = {}
        return cast(dict[str, object], answer) if isinstance(answer, dict) else {}

    def state(self) -> str:
        return str(self.cozy("status", "vault", "status").get("state"))

    def unlock(self, name: str, maximum: int = 0) -> Path:
        unlocked = self.cozy(name, "vault", "unlock", "--passphrase-file", str(self.passphrase), "--idle", "0", "--max-duration", str(maximum))
        vault = cast(dict[str, object], unlocked["vault"])
        assert vault["state"] == "unlocked", f"{name}: {unlocked}"
        return Path(str(vault["view"]))

    def start(self, name: str, script: Path, seconds: int, mark: str) -> tuple[subprocess.Popen[bytes], Path]:
        stdout = self.out / f"{name}.ndjson"
        process = subprocess.Popen([str(self.cli), "--json", "run", str(script), f"seconds:={seconds}", f"mark={mark}", "--on", "local"],
                                   cwd=self.project, stdout=stdout.open("wb"), stderr=(self.out / f"{name}.stderr").open("wb"))
        began = time.monotonic()
        while time.monotonic() - began < 180:
            if b'"started_at_ms"' in stdout.read_bytes():
                return process, stdout
            assert process.poll() is None, f"{name} ended before it started: {stdout.read_bytes()[-400:]!r}"
            time.sleep(0.2)
        raise AssertionError(f"{name} didn't start within 180 s")

    def status(self, number: object) -> str:
        done = subprocess.run([str(self.cli), "--json", "run", "watch", str(number), "--once"], cwd=self.project, capture_output=True)
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        try:
            watched: object = json.loads(lines[-1]) if lines else {}
        except ValueError:
            return ""
        if isinstance(watched, dict) and isinstance(watched.get("run"), dict):
            watched = watched["run"]
        return str(watched.get("status", "")) if isinstance(watched, dict) else ""


def number_of(stdout: Path) -> object:
    for line in reversed(stdout.read_bytes().splitlines()):
        try:
            event: object = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and isinstance(event.get("run"), dict):
            return cast(dict[str, object], event["run"]).get("number")
    raise AssertionError(f"no run in {stdout.name}")


def machine_pid() -> int:
    holders = list((Path.home() / ".cozy/machine").rglob("agent.lock.holder"))
    assert holders, "no local machine running"
    return int(json.loads(holders[0].read_text())["pid"])


def journal() -> list[Path]:
    """The machine's journal: the one store a stopped machine alone writes."""
    found = list((Path.home() / ".cozy/machine").rglob("executions.sqlite3"))
    assert found, "the local machine keeps no journal"
    return [found[0], found[0].with_name(found[0].name + "-wal"), found[0].with_name(found[0].name + "-shm")]


def journaled(view: Path) -> int | None:
    """The machine journal's runs of vault view `view`; None when it can't be read now."""
    try:
        connection = sqlite3.connect(f"file:{journal()[0]}?mode=ro", uri=True, timeout=2)
        try:
            row = connection.execute("SELECT COUNT(*) FROM executions WHERE json_extract(invocation,'$.local_private.view_root') = ?",
                                     (str(view),)).fetchone()
        finally:
            connection.close()
    except sqlite3.Error:
        return None
    return int(row[0])


def mounted(view: Path) -> bool:
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        point = line.split(" ")[4].replace("\\040", " ")
        if point == str(view):
            return True
    return False


def written(folder: Path, mark: str) -> list[str]:
    """Files under `folder` that hold `mark`."""
    found = []
    for path in folder.rglob("*") if folder.is_dir() else []:
        try:
            if path.is_file() and path.stat().st_size < 1 << 20 and mark.encode() in path.read_bytes():
                found.append(str(path))
        except OSError:
            continue
    return found


def traces(scanner: Scanner, out: Path, label: str, also_excluded: list[Path]) -> dict[str, object]:
    home = Path.home()
    store = Path(os.environ.get("TENSORFS_HOME", str(home / ".cache/tensorfs")))
    roots = [home / ".cozy", store, home / ".cache", home / ".config", home / ".local", Path("/tmp")]
    excluded = [home / ".cozy/calcifer/vaults", out.resolve(), *also_excluded]
    located = places(home, Path(os.environ.get("RUNNER_TEMP", "/tmp")))
    result: dict[str, object] = {}
    for _ in range(4):
        result = scanner.scan(roots, excluded, out / f"{label}-receipt.json", located, since_ns=SINCE)
        if not result["matches"]:
            break
        time.sleep(10)
    print(f"lock hung: {label}: {json.dumps({k: v for k, v in result.items() if k.startswith('match')})}", flush=True)
    return result


SINCE = time.time_ns()


def hung(cell: Cell, label: str, scanner: Scanner, view: Path, lock: object) -> dict[str, object]:
    """While the machine is stopped: the view gone, and nothing outside the folder but the machine's journal, named."""
    facts: dict[str, object] = {"mounted": mounted(view), "journal_rows": journaled(view), "journal": str(journal()[0])}
    facts["scan"] = traces(scanner, cell.out, f"{label}-stopped", [cell.project.resolve(), *journal()])
    print(f"lock hung: {label}: while local is stopped, the one exception is its own journal {facts['journal']}: "
          f"{facts['journal_rows']} runs of the ended view", flush=True)
    return facts


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    cell, facts = Cell(args), dict[str, object]()
    scanner = Scanner()
    scanner.identity(cell.project, args.project_id, cell.project.name)
    pid = machine_pid()
    try:
        if cell.state() != "locked":
            cell.cozy("lock-before", "vault", "lock")
        view = cell.unlock("unlock")
        script = view / "lock-hung" / "sleeper.py"
        script.parent.mkdir(exist_ok=True)
        script.write_text(SLEEPER)
        cell.cozy("warm", "run", str(script), "seconds:=0", "mark=warm", "--on", "local")

        # 1. The lock asked while local is stopped: it locks within its bound, says the clean-up is owed, and the view is gone.
        mark = f"CL1064_LOCK_{secrets.token_hex(12)}"
        scanner.add(mark)
        process, stdout = cell.start("stopped-run", script, SLEEPS, mark)
        started, number = time.monotonic(), number_of(stdout)
        os.kill(pid, signal.SIGSTOP)
        began = time.monotonic()
        done = subprocess.run([str(cell.cli), "--json", "vault", "lock"], cwd=cell.project, capture_output=True, timeout=BOUND * 3)
        (cell.out / "lock-stopped.json").write_bytes(done.stdout)
        (cell.out / "lock-stopped.stderr").write_bytes(done.stderr)
        while done.returncode == 0 and time.monotonic() - began < BOUND and cell.state() != "locked":
            time.sleep(1)
        facts["lock_s"] = round(time.monotonic() - began, 1)
        if done.returncode != 0 or cell.state() != "locked":
            # The release notes' workaround, as a person does it: the hung machine ended, then the lock asked again.
            facts["lock_failed"] = (done.stdout[-400:] + done.stderr[-400:]).decode(errors="replace")
            os.kill(pid, signal.SIGKILL)
            retried = subprocess.run([str(cell.cli), "--json", "vault", "lock"], cwd=cell.project, capture_output=True, timeout=BOUND * 3)
            facts["workaround"] = {"lock_exit": retried.returncode, "state": cell.state(), "mounted": mounted(view),
                                   "scan": traces(scanner, cell.out, "workaround", [cell.project.resolve()])}
            raise AssertionError(f"the lock failed while local was stopped: {facts['lock_failed']}; the workaround: {facts['workaround']}")
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        locked = cast(dict[str, object], json.loads(lines[-1])) if lines else {}
        facts["lock_said"] = locked.get("owed")
        facts["lock"] = hung(cell, "lock", scanner, view, locked)

        # 2. Unlocked again before local answers: the old view's run is canceled here, and stays so once local goes on.
        again = cell.unlock("unlock-again")
        assert again != view, f"unlocked again at the same view {view}"
        facts["canceled_before"] = cell.status(number)
        os.kill(pid, signal.SIGCONT)
        waited = time.monotonic()
        while time.monotonic() - waited < 90 and journaled(view):
            time.sleep(1)
        facts["journal_rows_after"] = journaled(view)
        facts["owed_paid_s"] = round(time.monotonic() - waited, 1)
        while time.monotonic() - started < SLEEPS + 20:
            time.sleep(1)
        facts["canceled_after"] = cell.status(number)
        facts["file_written"] = written(again / "files", mark) + written(again / ".vault-machine", mark)
        process.wait(timeout=60)
        cell.cozy("runs-again", "run", str(again / "lock-hung" / "sleeper.py"), "seconds:=0", "mark=again", "--on", "local")
        answered = cell.cozy("lock-answered", "vault", "lock")
        facts["answered_said"] = answered.get("owed")
        facts["after_lock"] = traces(scanner, cell.out, "lock-after", [cell.project.resolve()])

        # 3. The maximum's deadline while local is stopped: it locks past the maximum, and the same clean-up follows.
        mark = f"CL1064_MAX_{secrets.token_hex(12)}"
        scanner.add(mark)
        view = cell.unlock("unlock-maximum", MAXIMUM)
        unlocked_at = time.monotonic()
        process, stdout = cell.start("maximum-run", view / "lock-hung" / "sleeper.py", 300, mark)
        os.kill(pid, signal.SIGSTOP)
        while time.monotonic() - unlocked_at < MAXIMUM + BOUND and cell.state() != "locked":
            time.sleep(1)
        facts["maximum_locked_s"] = round(time.monotonic() - unlocked_at, 1)
        assert cell.state() == "locked", f"the vault is open {MAXIMUM + BOUND} s past a {MAXIMUM} s maximum while local is stopped"
        facts["maximum"] = hung(cell, "maximum", scanner, view, None)
        os.kill(pid, signal.SIGCONT)
        waited = time.monotonic()
        while time.monotonic() - waited < 90 and journaled(view):
            time.sleep(1)
        facts["maximum_journal_rows_after"] = journaled(view)
        process.wait(timeout=60)
        facts["after_maximum"] = traces(scanner, cell.out, "maximum-after", [cell.project.resolve()])
    finally:
        try:
            os.kill(pid, signal.SIGCONT)
        except ProcessLookupError:
            pass
        (args.out / "lock-hung.json").write_text(json.dumps(facts, indent=2, default=str))

    print(f"lock hung: {json.dumps({k: v for k, v in facts.items() if not isinstance(v, dict)})}", flush=True)
    failures = []
    lock, maximum = cast(dict[str, object], facts["lock"]), cast(dict[str, object], facts["maximum"])
    if cast(float, facts["lock_s"]) > BOUND:
        failures.append(f"the lock took {facts['lock_s']} s while local was stopped, past its {BOUND} s bound")
    if "didn't answer" not in str(facts["lock_said"]):
        failures.append(f"the lock didn't say local's clean-up is owed: {facts['lock_said']!r}")
    for label, stopped in (("lock", lock), ("maximum", maximum)):
        if stopped["mounted"]:
            failures.append(f"{label}: the view is still mounted while local is stopped")
        if cast(dict[str, object], stopped["scan"])["matches"]:
            failures.append(f"{label}: while local is stopped, the project is outside its folder beyond the machine's journal: {stopped['scan']}")
    if facts["canceled_before"] != "canceled" or facts["canceled_after"] != "canceled":
        failures.append(f"the stopped run was {facts['canceled_before']} when unlocked again and {facts['canceled_after']} after local went on")
    if facts["file_written"]:
        failures.append(f"the stopped run's file appeared after local went on: {facts['file_written']}")
    if facts["journal_rows_after"] or facts["maximum_journal_rows_after"]:
        failures.append(f"local's journal keeps the ended views' runs after it answered: {facts['journal_rows_after']}, {facts['maximum_journal_rows_after']}")
    if facts["answered_said"]:
        failures.append(f"a lock local answered said its clean-up is owed: {facts['answered_said']!r}")
    for label in ("after_lock", "after_maximum"):
        if cast(dict[str, object], facts[label])["matches"]:
            failures.append(f"{label}: the project is outside its folder once local answered: {facts[label]}")
    assert not failures, "lock hung: " + "; ".join(failures)
    print(f"lock hung: locked in {facts['lock_s']} s and {facts['maximum_locked_s']} s (maximum {MAXIMUM} s) with local stopped; "
          f"its clean-up done {facts['owed_paid_s']} s after it went on", flush=True)


if __name__ == "__main__":
    main()
