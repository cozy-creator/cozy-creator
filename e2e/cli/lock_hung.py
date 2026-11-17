#!/usr/bin/env python3
"""lock_hung (cl-1064): an encrypted project locks while its local machine doesn't answer. Paul: "lock must succeed when asked".

The local machine's service is stopped (SIGSTOP), as a hung GPU machine is: the processes that answer its requests, never its
stable parent, which only holds the listeners (tensord machine/supervise.rs). Each way it hangs must lock within its bound, its view
gone, nothing of the project anywhere outside its folder but the machine's own journal, which only the stopped machine writes: the
one exception, named. Once the machine goes on (SIGCONT), what it held of that view is cancelled and retired before anything else.

1. `cozy vault lock`, stopped as a run's output starts coming into the project (the machine holds the project's output): it locks
   and says local's clean-up is owed. Unlocked again before the machine answers; once it does, nothing the lock settled changes, no
   file of the run appears, and the project runs again.
2. The maximum's deadline, stopped the same way.
3. The maximum's deadline, stopped while a run's call is going, in a second project: the run holds the vault past its maximum until
   the clock asks local, which doesn't answer; the vault locks and the run settles canceled.
4. A brief hang (a 15 s maximum, past before the run's output starts coming in): local stopped as the output starts, until the clock
   has judged it gone and the vault is locking, then going on at once (back mid-drain). Its save goes on where it was, judged by its
   progress, so it lands and the run reads completed.
5. Local slow from the moment the output starts coming (1.5 s stopped, 0.5 s going): slow, each status ping still answered within
   its 2 s, its save's bytes still coming. It is never judged gone, its save lands, and the vault locks once it has (re-review
   6939581151).

A step that doesn't lock takes the release note's workaround, as a person does (the hung machine ended, the lock asked again), and
the cell goes on: a red names each way it hangs. The machine always goes on at the end."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent / "containment"))
from scan import Scanner, places  # noqa: E402

MAXIMUM = 30
# Step 2's maximum: its run writes and hashes a 2 GB output before it starts coming in, so the machine stops before the deadline.
MAXIMUM_SAVING = 60
# The lock's bound: a save gone silent is cut (outputs.rs EXPORT_SILENCE, 20 s), three unanswered asks of local (a second apart,
# two seconds each), and the lock itself.
BOUND = 60
# A deadline's bound past the maximum (coordinator 11-17: "a deadline lock must lock close to its deadline"): local asked three
# times, a save from it cut at once, and the lock.
DEADLINE_BOUND = 15
# Big enough that its save into the project takes seconds, so the stop lands in it (as vault_work's step 4).
SAVE_MB = 2048
SLEEPER = '''# /// script
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
    mark: str
    size: int = 0
class Answer(msgspec.Struct):
    file: File
@app.entrypoint(accelerator=False)
def lock_hung_sleep(payload: Request, out: Outputs) -> Answer:
    time.sleep(payload.seconds)
    target = out.temporary_file(".txt")
    with open(target, "wb") as file:
        file.write(f"{payload.mark}\\n".encode())
        for _ in range(payload.size):
            file.write(os.urandom(1 << 20))
    return Answer(out.save_file(target, media_type="text/plain"))
'''


class Cell:
    def __init__(self, args: argparse.Namespace, project: Path, tag: str) -> None:
        self.cli: Path = args.cli
        self.out: Path = args.out
        self.passphrase: Path = args.passphrase_file
        self.project = project
        self.tag = tag

    def cozy(self, name: str, *words: str, fails: bool = False, timeout: float = 300) -> dict[str, object]:
        done = subprocess.run([str(self.cli), "--json", *words], cwd=self.project, capture_output=True, timeout=timeout)
        (self.out / f"{self.tag}{name}.json").write_bytes(done.stdout)
        (self.out / f"{self.tag}{name}.stderr").write_bytes(done.stderr)
        assert bool(done.returncode) == fails, f"{self.tag}{name}: exit {done.returncode}: {done.stdout[-500:]!r} {done.stderr[-500:]!r}"
        return last(done.stdout)

    def state(self) -> str:
        done = subprocess.run([str(self.cli), "--json", "vault", "status"], cwd=self.project, capture_output=True, timeout=60)
        return str(last(done.stdout).get("state"))

    def unlock(self, name: str, maximum: int = 0) -> Path:
        unlocked = self.cozy(name, "vault", "unlock", "--passphrase-file", str(self.passphrase), "--idle", "0", "--max-duration", str(maximum))
        vault = cast(dict[str, object], unlocked["vault"])
        assert vault["state"] == "unlocked", f"{self.tag}{name}: {unlocked}"
        return Path(str(vault["view"]))

    def start(self, name: str, script: Path, seconds: int, mark: str, size: int = 0) -> tuple[subprocess.Popen[bytes], Path]:
        stdout = self.out / f"{self.tag}{name}.ndjson"
        words = [str(self.cli), "--json", "run", str(script), f"seconds:={seconds}", f"mark={mark}", *([f"size:={size}"] if size else []), "--on", "local"]
        process = subprocess.Popen(words, cwd=self.project, stdout=stdout.open("wb"), stderr=(self.out / f"{self.tag}{name}.stderr").open("wb"))
        return process, stdout

    def status(self, number: object) -> str:
        """Run `number`'s status, and its reason's code when it has one."""
        done = subprocess.run([str(self.cli), "--json", "run", "watch", str(number), "--once"], cwd=self.project, capture_output=True, timeout=60)
        watched: object = last(done.stdout)
        if isinstance(watched, dict) and isinstance(watched.get("run"), dict):
            watched = watched["run"]
        if not isinstance(watched, dict):
            return ""
        reason = watched.get("reason") or watched.get("error")
        code = reason.get("code") if isinstance(reason, dict) else None
        return f"{watched.get('status', '')}{f' ({code})' if code else ''}"

    def lock(self, name: str) -> tuple[float, dict[str, object], str]:
        """`cozy vault lock`, then the vault seen locked: its seconds, its answer, and what went wrong if it didn't lock."""
        began = time.monotonic()
        try:
            done = subprocess.run([str(self.cli), "--json", "vault", "lock"], cwd=self.project, capture_output=True, timeout=BOUND * 3)
        except subprocess.TimeoutExpired as expired:
            (self.out / f"{self.tag}{name}.json").write_bytes(expired.stdout or b"")
            return round(time.monotonic() - began, 1), {}, f"cozy vault lock didn't answer within {BOUND * 3} s"
        (self.out / f"{self.tag}{name}.json").write_bytes(done.stdout)
        (self.out / f"{self.tag}{name}.stderr").write_bytes(done.stderr)
        while done.returncode == 0 and time.monotonic() - began < BOUND and self.state() != "locked":
            time.sleep(1)
        seconds = round(time.monotonic() - began, 1)
        if done.returncode != 0 or self.state() != "locked":
            said = b" ".join((done.stdout[-400:] + done.stderr[-400:]).split()).decode(errors="replace")
            return seconds, {}, said or f"still {self.state()}"
        return seconds, last(done.stdout), ""

    def locked_by(self, since: float, seconds: float) -> float | None:
        """Seconds from `since` until the vault reads locked; None if it doesn't within `seconds`."""
        while time.monotonic() - since < seconds:
            if self.state() == "locked":
                return round(time.monotonic() - since, 1)
            time.sleep(1)
        return None

    def workaround(self, label: str, scanner: Scanner, view: Path) -> dict[str, object]:
        """The release note's workaround, as a person does it: the hung machine ended, then the lock asked again."""
        for pid in list(STOPPED):
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        STOPPED.clear()
        retried = subprocess.run([str(self.cli), "--json", "vault", "lock"], cwd=self.project, capture_output=True, timeout=BOUND * 3)
        return {"lock_exit": retried.returncode, "state": self.state(), "mounted": mounted(view),
                "scan": traces(scanner, self.out, f"{self.tag}{label}-workaround", [self.project.resolve()])}


def last(stdout: bytes) -> dict[str, object]:
    lines = [line for line in stdout.splitlines() if line.strip()]
    try:
        answer: object = json.loads(lines[-1]) if lines else {}
    except ValueError:
        answer = {}
    return cast(dict[str, object], answer) if isinstance(answer, dict) else {}


def sleeper(view: Path) -> Path:
    script = view / "lock-hung" / "sleeper.py"
    script.parent.mkdir(exist_ok=True)
    script.write_text(SLEEPER)
    return script


def number_of(stdout: Path) -> object:
    for line in reversed(stdout.read_bytes().splitlines()):
        try:
            event: object = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and isinstance(event.get("run"), dict):
            return cast(dict[str, object], event["run"]).get("number")
    raise AssertionError(f"no run in {stdout.name}")


def waiting(stdout: Path, process: subprocess.Popen[bytes], seen: re.Pattern[bytes], seconds: float = 300) -> bool:
    """Until the run's events show `seen`, or it ends, or `seconds` pass."""
    began = time.monotonic()
    while process.poll() is None and time.monotonic() - began < seconds:
        if seen.search(stdout.read_bytes()):
            return True
        time.sleep(0.02)
    return bool(seen.search(stdout.read_bytes()))


# Its output starts coming into the project: its call ended (outputs are saved as it returns). Or its call began.
ARRIVING = re.compile(rb'"output_item\.added"')
BEGUN = re.compile(rb'"started_at_ms":\s*\d')


def services() -> list[int]:
    """The local machine's service: what answers its requests. The lock's holder is its stable parent, which only holds the
    listeners and forks the service (tensord machine/supervise.rs): stopping that parent stops nothing that answers."""
    holder = Path.home() / ".cozy/machine/var/lib/cozy/machine/agent.lock.holder"
    assert holder.is_file(), "no local machine running"
    parent = int(json.loads(holder.read_text())["pid"])
    found = []
    for stat in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = stat.read_text().rsplit(")", 1)[1].split()
            command = (stat.parent / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        if int(fields[1]) == parent and "serve" in command:
            found.append(int(stat.parent.name))
    assert found, f"no service under the local machine's parent {parent}"
    return found


STOPPED: list[int] = []


def stop(view: Path) -> dict[str, object]:
    """The local machine's service stopped (SIGSTOP), as a hung one is, each seen stopped; and whether it holds the project's
    output in its store in the view."""
    stopped: dict[str, object] = {}
    for pid in services():
        os.kill(pid, signal.SIGSTOP)
        STOPPED.append(pid)
        state = ""
        for _ in range(50):
            state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
            if state == "T":
                break
            time.sleep(0.1)
        assert state == "T", f"the local machine's service {pid} isn't stopped: {state}"
        stopped[str(pid)] = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
    store = view / ".vault-machine"
    stopped["holds_output"] = store.is_dir() and any(path.is_file() for path in store.rglob("*"))
    return stopped


def go_on() -> None:
    while STOPPED:
        try:
            os.kill(STOPPED.pop(), signal.SIGCONT)
        except ProcessLookupError:
            pass


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


def left(view: Path) -> list[object]:
    """The machine journal's runs of `view` still there: each one's id and state, for a red."""
    try:
        connection = sqlite3.connect(f"file:{journal()[0]}?mode=ro", uri=True, timeout=2)
        try:
            return [list(row) for row in connection.execute(
                "SELECT id, state, json_extract(invocation,'$.local_private.project_id') IS NOT NULL FROM executions "
                "WHERE json_extract(invocation,'$.local_private.view_root') = ?", (str(view),))]
        finally:
            connection.close()
    except sqlite3.Error as error:
        return [str(error)]


def paid(view: Path) -> tuple[int | None, float]:
    """Once the machine goes on: its journal's runs of `view` once they're gone (or after 90 s), and the seconds it took."""
    began = time.monotonic()
    rows = journaled(view)
    while rows != 0 and time.monotonic() - began < 90:
        time.sleep(1)
        rows = journaled(view)
    return rows, round(time.monotonic() - began, 1)


def executors_of(view: Path) -> list[int] | None:
    """The processes whose environment names `view`: a project's executors (their home and scratch are in its view). None where
    this OS's processes' environments aren't read here (only Linux's are)."""
    if not sys.platform.startswith("linux"):
        return None
    needle, found = str(view).encode(), []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        try:
            environ = (entry / "environ").read_bytes()
        except OSError:
            continue
        if any(value.split(b"=", 1)[-1].startswith(needle) for value in environ.split(b"\0")):
            found.append(int(entry.name))
    return found


def executors_but(view: Path) -> dict[int, str]:
    """Executors on this machine (Linux) of everything but `view`, each with its start (so a reused pid isn't taken for it)."""
    if not sys.platform.startswith("linux"):
        return {}
    ours = set(executors_of(view) or [])
    found = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit() or int(entry.name) in ours:
            continue
        try:
            if b"cozy_runtime.internal.executor" in (entry / "cmdline").read_bytes():
                found[int(entry.name)] = (entry / "stat").read_text().rsplit(")", 1)[1].split()[19]
        except OSError:
            continue
    return found


def alive(pid: int, start: str) -> bool:
    """Whether process `pid`, started at `start`, still runs (a zombie has ended)."""
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    except OSError:
        return False
    return fields[19] == start and fields[0] != "Z"


def executors_gone(view: Path) -> list[int] | None:
    """`executors_of(view)` once they had 10 s to go."""
    began, left = time.monotonic(), executors_of(view)
    while left and time.monotonic() - began < 10:
        time.sleep(0.5)
        left = executors_of(view)
    return left


BIRTHS = Path.home() / ".cozy/machine/var/lib/cozy/tensord/private-executors"


def births_kept(view: Path) -> bool:
    """Whether local keeps its executors' births for `view` (cl-1064): never past the view's close or its owed clean-up."""
    return (BIRTHS / hashlib.sha256(str(view).encode()).hexdigest()).exists()


def live_mounts() -> set[str]:
    return {line.split(" ")[4].replace("\\040", " ") for line in Path("/proc/self/mountinfo").read_text().splitlines()}


def runtime_left(folder: Path, ended: Path, mark: str, seconds: float = 30) -> list[str]:
    """Once local answered and its clean-up is paid: what of the ended view stays in the project's folder (review 6936314732). Its
    sealed mount point stays only until then; the runtime area holds nothing else outside a live mount (its `owner` mark aside); and
    nothing in the folder, its encrypted backing aside, holds the run's mark."""
    began = time.monotonic()
    while ended.parent.exists() and time.monotonic() - began < seconds:
        time.sleep(1)
    found = [f"{ended.parent.name}: still there"] if ended.parent.exists() else []
    live = live_mounts()
    for top, folders, files in os.walk(folder):
        here = Path(top)
        folders[:] = [name for name in folders if str(here / name) not in live and not (here == folder and name == "project.vault.data")]
        for name in files:
            path = here / name
            if path.parent == folder / ".vault-runtime" and name == "owner":
                continue
            inside_runtime = folder / ".vault-runtime" in path.parents
            try:
                held = path.stat().st_size < 64 << 20 and mark.encode() in path.read_bytes()
            except OSError:
                held = False
            if inside_runtime or held:
                found.append(f"{path.relative_to(folder)}{' (holds the mark)' if held else ''}")
    return found


# Steps 4 and 5's output, in MB: the stop lands as its save starts, so a small one still stops mid-save.
SMALL_SAVE_MB = 512
# Step 4's maximum: past before its output starts coming in, so the clock judges local gone soon after the stop, and local is back
# while its save's read is still open (a hang of seconds, not of the minute after which a read gives up and pauses).
MAXIMUM_BACK = 15


def until_state(cell: Cell, wanted: str, seconds: float) -> float | None:
    """Seconds until the vault reads `wanted` (looked at as often as its status answers), or None."""
    began = time.monotonic()
    while time.monotonic() - began < seconds:
        if cell.state() == wanted:
            return round(time.monotonic() - began, 1)
        time.sleep(0.1)
    return None


def slow_local(going: threading.Event) -> None:
    """Local stopped and going on in turn until `going` is set: 1.5 s stopped, 0.5 s going, so most of its status pings (2 s
    each) go unanswered while a save's bytes still come every 2 s."""
    while not going.is_set():
        for pid in STOPPED:
            try:
                os.kill(pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        time.sleep(0.5)
        for pid in STOPPED:
            try:
                os.kill(pid, signal.SIGSTOP)
            except ProcessLookupError:
                pass
        going.wait(1.5)


def judged_gone_then_back(cell: Cell, label: str, slow: bool, maximum: int, facts: dict[str, object], scanner: Scanner) -> list[str]:
    """Step 4 (`slow` False): the deadline with a run's output coming in, local stopped until the clock judged it gone (the vault
    reads locking), then going on at once. Step 5 (`slow`): local slow from the output's start, never judged gone. In both the save
    lands and the run reads completed."""
    failures = []
    mark = f"CL1064_{label.upper()}_{secrets.token_hex(12)}"
    scanner.add(mark)
    if cell.state() != "locked":
        cell.cozy(f"lock-before-{label}", "vault", "lock")
    view = cell.unlock(f"unlock-{label}", maximum)
    process, stdout = cell.start(f"{label}-run", sleeper(view), 1, mark, SMALL_SAVE_MB)
    assert waiting(stdout, process, ARRIVING), f"{label}: the run's output never started coming: {stdout.read_bytes()[-400:]!r}"
    number = number_of(stdout)
    facts[f"{label}_stopped"] = stop(view)
    going = threading.Event()
    if slow:
        threading.Thread(target=slow_local, args=(going,), daemon=True).start()
        judged = None
    else:
        judged = facts[f"{label}_judged_s"] = until_state(cell, "locking", maximum + DEADLINE_BOUND + 30)
        go_on()
    locked = facts[f"{label}_locked_after_s"] = cell.locked_by(time.monotonic(), maximum + 180)
    going.set()
    time.sleep(0.6)
    go_on()
    finish(process)
    if judged is None and not slow:
        failures.append(f"{label}: the clock never judged local gone (the vault never read locking)")
    if locked is None:
        failures.append(f"{label}: the vault never locked once local was back")
        return failures
    again = cell.unlock(f"unlock-after-{label}")
    run = facts[f"{label}_run"] = cell.status(number)
    files = facts[f"{label}_files"] = written(again / "files", mark)
    cell.cozy(f"lock-after-{label}", "vault", "lock")
    if not str(run).startswith("completed") or not files:
        failures.append(f"{label}: a save whose bytes still came was cut: the run is {run!r}, its file {files or 'absent'}")
    return failures


def deadline_of(unlocked_at: float, maximum: int, facts: dict[str, object], label: str) -> float:
    """When the vault's deadline passes with local stopped: its maximum, or the machine's stop if that came later (until then its
    run held the vault, as runs do). The lock is measured from it."""
    stopped_at = time.monotonic()
    deadline = max(unlocked_at + maximum, stopped_at)
    facts[f"{label}_deadline"] = "its maximum" if deadline > stopped_at else f"the stop, {round(stopped_at - unlocked_at, 1)} s after unlocking (past the {maximum} s maximum)"
    return deadline


def mounted(view: Path) -> bool:
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        point = line.split(" ")[4].replace("\\040", " ")
        if point == str(view):
            return True
    return False


def written(folder: Path, mark: str) -> list[str]:
    """Files under `folder` that begin with `mark`, whole or partly written."""
    found = []
    for path in folder.rglob("*") if folder.is_dir() else []:
        try:
            if path.is_file():
                with path.open("rb") as file:
                    if file.read(len(mark) + 1).startswith(mark.encode()):
                        found.append(str(path))
        except OSError:
            continue
    return sorted(found)


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


def hung(cell: Cell, label: str, scanner: Scanner, view: Path, projects: list[Path]) -> dict[str, object]:
    """While the machine is stopped: the view gone, and nothing outside the folders but the machine's journal, named."""
    facts: dict[str, object] = {"mounted": mounted(view), "journal_rows": journaled(view), "journal": str(journal()[0]),
                                "executors_left": executors_gone(view), "births_kept_dir": BIRTHS.is_dir()}
    facts["scan"] = traces(scanner, cell.out, f"{label}-stopped", [*(project.resolve() for project in projects), *journal()])
    print(f"lock hung: {label}: while local is stopped, the one exception is its own journal {facts['journal']}: "
          f"{facts['journal_rows']} runs of the ended view", flush=True)
    return facts


def finish(process: subprocess.Popen[bytes]) -> None:
    try:
        process.wait(timeout=120)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=30)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--second", required=True, type=Path, help="where step 3's project is made")
    parser.add_argument("--recovery-out", required=True, type=Path, help="its recovery secret")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    first = Cell(args, args.project, "")
    facts: dict[str, object] = {}
    failures: list[str] = []
    scanner = Scanner()
    scanner.identity(first.project, args.project_id, first.project.name)
    projects = [first.project]
    try:
        if first.state() != "locked":
            first.cozy("lock-before", "vault", "lock")
        view = first.unlock("unlock")
        first.cozy("warm", "run", str(sleeper(view)), "seconds:=0", "mark=warm", "--on", "local")

        # 1. The lock asked while local is stopped as a run's output starts coming: it locks within its bound, says the clean-up is
        #    owed, and the view is gone.
        mark = f"CL1064_LOCK_{secrets.token_hex(12)}"
        scanner.add(mark)
        process, stdout = first.start("lock-run", sleeper(view), 1, mark, SAVE_MB)
        assert waiting(stdout, process, ARRIVING), f"the run's output never started coming: {stdout.read_bytes()[-400:]!r}"
        number = number_of(stdout)
        facts["lock_stopped"] = stop(view)
        facts["lock_s"], locked, failed = first.lock("lock-stopped")
        if failed:
            failures.append(f"the lock failed while local was stopped ({facts['lock_s']} s): {failed}")
            facts["lock_workaround"] = first.workaround("lock", scanner, view)
            go_on()
            finish(process)
        else:
            if cast(float, facts["lock_s"]) > BOUND:
                failures.append(f"the lock took {facts['lock_s']} s while local was stopped, past its {BOUND} s bound")
            facts["lock_said"] = locked.get("owed")
            if "didn't answer" not in str(facts["lock_said"]):
                failures.append(f"the lock didn't say local's clean-up is owed: {facts['lock_said']!r}")
            if "ended the project's" not in str(facts["lock_said"]):
                failures.append(f"the lock didn't say it ended the project's processes on local: {facts['lock_said']!r}")
            facts["lock"] = hung(first, "lock", scanner, view, projects)

            # Unlocked again before local answers: once it goes on, nothing the lock settled changes, nothing of the run appears in
            # the project, the machine forgets the ended view, and the project runs again.
            again = first.unlock("unlock-again")
            assert again != view, f"unlocked again at the same view {view}"
            facts["run_before"] = first.status(number)
            facts["files_before"] = written(again / "files", mark) + written(again / ".vault-machine", mark)
            go_on()
            facts["journal_rows_after"], facts["owed_paid_s"] = paid(view)
            facts["births_after_paid"] = births_kept(view)
            facts["runtime_left"] = runtime_left(first.project, view, mark)
            finish(process)
            facts["run_after"] = first.status(number)
            facts["files_after"] = written(again / "files", mark) + written(again / ".vault-machine", mark)
            first.cozy("runs-again", "run", str(sleeper(again)), "seconds:=0", "mark=again", "--on", "local")
            facts["answered_said"] = first.cozy("lock-answered", "vault", "lock").get("owed")
            facts["births_after_lock"] = births_kept(again)
            facts["after_lock"] = traces(scanner, args.out, "lock-after", projects)
            if str(facts["run_before"]).split(" ")[0] not in ("canceled", "failed", "completed") or facts["run_after"] != facts["run_before"]:
                failures.append(f"the stopped run was {facts['run_before']!r} when unlocked again and {facts['run_after']!r} once local went on")
            if facts["files_after"] != facts["files_before"]:
                failures.append(f"the stopped run wrote into the project once local went on: {facts['files_before']} became {facts['files_after']}")
            if facts["journal_rows_after"] != 0:
                failures.append(f"local's journal keeps the ended view's runs once it answered: {facts['journal_rows_after']}")
            if facts["runtime_left"]:
                failures.append(f"lock: the ended view stays in the project's folder once local answered: {facts['runtime_left']}")
            if facts["answered_said"]:
                failures.append(f"a lock local answered said its clean-up is owed: {facts['answered_said']!r}")
            if facts["births_after_paid"] or facts["births_after_lock"]:
                failures.append(f"local keeps a view's executor births past its clean-up ({facts['births_after_paid']}) or its lock ({facts['births_after_lock']})")
            lock_facts = cast(dict[str, object], facts["lock"])
            if lock_facts["executors_left"]:
                failures.append(f"lock: the project's executors still run on local after its lock: {lock_facts['executors_left']}")
            if not lock_facts["births_kept_dir"]:
                failures.append("local keeps no executor births (the lock fell back to ending its whole service)")
            for label in ("lock", "after_lock"):
                found = cast(dict[str, object], facts[label])
                if label == "lock" and found["mounted"]:
                    failures.append("lock: the view is still mounted while local is stopped")
                scan = cast(dict[str, object], found["scan"]) if label == "lock" else found
                if scan["matches"]:
                    failures.append(f"{label}: the project is outside its folder: {scan}")

        # 2. The maximum's deadline while local is stopped as a run's output starts coming: it locks past the maximum, within its
        #    bound, and the same clean-up follows.
        mark = f"CL1064_MAX_{secrets.token_hex(12)}"
        scanner.add(mark)
        if first.state() != "locked":
            first.cozy("lock-before-maximum", "vault", "lock")
        view = first.unlock("unlock-maximum", MAXIMUM_SAVING)
        unlocked_at = time.monotonic()
        process, stdout = first.start("maximum-run", sleeper(view), 1, mark, SAVE_MB)
        assert waiting(stdout, process, ARRIVING), f"the maximum's run's output never started coming: {stdout.read_bytes()[-400:]!r}"
        facts["maximum_stopped"] = stop(view)
        facts["maximum_locked_s"] = first.locked_by(deadline_of(unlocked_at, MAXIMUM_SAVING, facts, "maximum"), DEADLINE_BOUND)
        failures.extend(deadline(first, "maximum", scanner, view, projects, facts, process, mark))

        # 4 and 5. Judged gone, then back (at once, or slow): a save whose bytes still come lands, and its run reads completed.
        failures.extend(judged_gone_then_back(first, "back", slow=False, maximum=MAXIMUM_BACK, facts=facts, scanner=scanner))
        failures.extend(judged_gone_then_back(first, "slow", slow=True, maximum=MAXIMUM_SAVING, facts=facts, scanner=scanner))

        # 3. The maximum's deadline while local is stopped mid-call, in a second project: the run holds the vault past its maximum
        #    until the clock asks local, which doesn't answer; the vault locks and the run settles canceled.
        made = subprocess.run([str(args.cli), "--json", "new", str(args.second), "--passphrase-file", str(args.passphrase_file),
                               "--recovery-out", str(args.recovery_out)], capture_output=True, timeout=300)
        assert made.returncode == 0, f"cozy new {args.second}: {made.stdout[-300:]!r} {made.stderr[-300:]!r}"
        second = Cell(args, args.second, "second-")
        scanner.identity(second.project, str(last(made.stdout).get("id")), second.project.name)
        projects.append(second.project)
        if second.state() != "locked":
            second.cozy("lock-before", "vault", "lock")
        view = second.unlock("unlock")
        second.cozy("warm", "run", str(sleeper(view)), "seconds:=0", "mark=warm", "--on", "local")
        second.cozy("lock-warm", "vault", "lock")
        mark = f"CL1064_GOING_{secrets.token_hex(12)}"
        scanner.add(mark)
        view = second.unlock("unlock-maximum", MAXIMUM)
        unlocked_at = time.monotonic()
        # Another project's executor on local, warm from a render (local's CPU takes one call at a time, so not mid-call): the lock
        # ends only the locked project's processes, so it lives on, and the project renders again once local answers.
        plain_folder = Path(tempfile.mkdtemp(prefix="lock-hung-plain-")) / "plain"
        made = subprocess.run([str(args.cli), "--json", "new", "--unencrypted", str(plain_folder)], capture_output=True, timeout=300)
        assert made.returncode == 0, f"cozy new --unencrypted: {made.stdout[-300:]!r} {made.stderr[-300:]!r}"
        plain = Cell(args, plain_folder, "plain-")
        plain.cozy("plain-warm", "run", str(sleeper(plain_folder)), "seconds:=0", "mark=plain", "--on", "local")
        process, stdout = second.start("going-run", sleeper(view), 600, mark)
        assert waiting(stdout, process, BEGUN, 180), f"the going run didn't begin: {stdout.read_bytes()[-400:]!r}"
        number = number_of(stdout)
        others = facts["other_executors"] = executors_but(view)
        facts["going_stopped"] = stop(view)
        facts["going_locked_s"] = second.locked_by(deadline_of(unlocked_at, MAXIMUM, facts, "going"), DEADLINE_BOUND)
        if facts["going_locked_s"] is not None:
            ended = facts["other_executors_ended"] = [pid for pid, start in others.items() if not alive(pid, start)]
            if not others:
                failures.append("going: no other project's executor was warm on local before the stop, so none could be seen to live on")
            elif ended:
                failures.append(f"going: the lock ended another project's executors on local: {ended}")
        failures.extend(deadline(second, "going", scanner, view, projects, facts, process, mark))
        # Its run read once local answered, from the project unlocked again: a locked project's runs aren't read.
        if second.state() != "locked":
            second.cozy("lock-before-reading", "vault", "lock")
        second.unlock("unlock-to-read")
        facts["going_run"] = second.status(number)
        second.cozy("lock-after-reading", "vault", "lock")
        if facts["going_locked_s"] is not None and not str(facts["going_run"]).startswith("canceled"):
            failures.append(f"going: the run going when local stopped is {facts['going_run']!r} once the vault locked, not canceled")
        rendered = subprocess.run([str(args.cli), "--json", "run", str(sleeper(plain_folder)), "seconds:=1", "mark=plain-after", "--on", "local"],
                                  cwd=plain_folder, capture_output=True, timeout=600)
        answer = last(rendered.stdout)
        facts["plain_after"] = cast(dict[str, object], answer.get("run", answer)).get("status") if rendered.returncode == 0 else rendered.stderr[-300:]
        if facts["plain_after"] != "completed":
            failures.append(f"another project doesn't render on local once it answered: {facts['plain_after']!r}")
    except BaseException:
        for project in projects:
            clock(project)
        raise
    finally:
        go_on()
        (args.out / "lock-hung.json").write_text(json.dumps(facts, indent=2, default=str))

    print(f"lock hung: {json.dumps({k: v for k, v in facts.items() if not isinstance(v, dict)})}", flush=True)
    if failures:
        for project in projects:
            clock(project)
    assert not failures, "lock hung: " + "; ".join(failures)
    print(f"lock hung: locked in {facts['lock_s']} s by hand, and {facts['maximum_locked_s']} s and {facts['going_locked_s']} s past "
          f"a deadline (bound {DEADLINE_BOUND} s), with local stopped; its clean-up done {facts['owed_paid_s']} s after it went on", flush=True)


def clock(project: Path) -> None:
    """What Calcifer said of `project` in its diagnostics (on for this shard): why its vault stayed open, what a lock met."""
    log = Path.home() / ".cozy/calcifer/diagnostics.log"
    lines = [line for line in log.read_text(errors="replace").splitlines() if str(project) in line] if log.is_file() else []
    print(f"lock hung: the clock on {project.name}: " + (" | ".join(lines[-30:]) or "nothing (no diagnostics log)"), flush=True)


def deadline(cell: Cell, label: str, scanner: Scanner, view: Path, projects: list[Path], facts: dict[str, object],
             process: subprocess.Popen[bytes], mark: str) -> list[str]:
    """A deadline step once its vault locked or didn't: what's wrong with it, the machine gone on and its clean-up seen done."""
    failures = []
    if facts[f"{label}_locked_s"] is None:
        failures.append(f"{label}: the vault is open {DEADLINE_BOUND} s past its deadline ({facts[f'{label}_deadline']}) while local is stopped")
        facts[f"{label}_workaround"] = cell.workaround(label, scanner, view)
    else:
        found = facts[label] = hung(cell, label, scanner, view, projects)
        if found["executors_left"]:
            failures.append(f"{label}: the project's executors still run on local after its lock: {found['executors_left']}")
        if found["mounted"]:
            failures.append(f"{label}: the view is still mounted while local is stopped")
        if cast(dict[str, object], found["scan"])["matches"]:
            failures.append(f"{label}: while local is stopped, the project is outside its folder beyond the machine's journal: {found['scan']}")
    go_on()
    rows, _ = paid(view)
    facts[f"{label}_journal_rows_after"] = rows
    if births_kept(view):
        failures.append(f"{label}: local keeps the view's executor births past its clean-up")
    remains = facts[f"{label}_runtime_left"] = runtime_left(cell.project, view, mark)
    if remains:
        failures.append(f"{label}: the ended view stays in the project's folder once local answered: {remains}")
    if rows != 0:
        failures.append(f"{label}: local's journal keeps the ended view's runs once it answered: {rows} (id, state, private: {left(view)})")
    finish(process)
    after = facts[f"after_{label}"] = traces(scanner, cell.out, f"{label}-after", projects)
    if after["matches"]:
        failures.append(f"after_{label}: the project is outside its folder once local answered: {after}")
    return failures


if __name__ == "__main__":
    main()
