#!/usr/bin/env python3
"""folder_gone (cl-1124): an unlocked encrypted project's folder removed or moved from outside, as a person's file manager would.

A removal is a delete, on every OS: the folder goes within 20 s of retries (on Linux and macOS its mounted view blocks the first pass,
until Calcifer ends the session and takes away what it kept there). Then, each within its bound: `cozy vault lock` on it answers and
says the folder is gone; its idle lock ends the session; a folder moved away (to the Trash) leaves no view mounted and can be removed
from where it went; Calcifer answers all along; and `cozy restart`, which lets go of every view, ends and succeeds. Its keybag alone
deleted, the session stays open: it holds the only copy of the keys."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from typing import cast

# A lock asked of it, and Calcifer's status, answer within this; a restart within `RESTART`.
ANSWER = 60
RESTART = 120
def ended(cell: "Cell", folder: Path, view: str) -> dict[str, object]:
    """A session whose folder's keys and data went (Linux, macOS: its mounted view keeps the folder): its state once it ended, or
    as it stays `ANSWER` s on."""
    began = time.monotonic()
    state = cell.state(folder)
    while state in ("unlocked", "locking", "no answer") and time.monotonic() - began < ANSWER:
        time.sleep(1)
        state = cell.state(folder)
    return {"state": state, "ended_s": round(time.monotonic() - began, 1), "mounted_after": mounted(view)}


# The idle lock's limit, and how long past it the session may last.
IDLE = 15
IDLE_BOUND = 60
# A view moved with its folder is unmounted within this.
MOVED_BOUND = 30


def mounted(view: str) -> bool:
    """Whether an encrypted view is still mounted, as this OS's mount table says (Windows: its drive letter)."""
    if sys.platform == "win32":
        return Path(view).exists()
    if sys.platform == "darwin":
        table = subprocess.run(["mount"], capture_output=True, text=True).stdout
        return any(f" on {view} (" in line or f" on /private{view} (" in line for line in table.splitlines())
    escaped = view.replace("\\", "\\134").replace(" ", "\\040")
    return any(line.split()[4] == escaped for line in Path("/proc/self/mountinfo").read_text().splitlines())


def answer(said: str) -> dict[str, object]:
    """The last JSON object `cozy --json` printed: its answer."""
    for line in reversed(said.splitlines()):
        try:
            found = json.loads(line)
        except ValueError:
            continue
        if isinstance(found, dict):
            return cast(dict[str, object], found)
    return {}


def left(folder: Path) -> list[str]:
    """What stays of `folder` after a removal failed: each path, its mode and kind (at most 30), and mounts under it (macOS, Linux)."""
    found: list[str] = []
    for path in [folder, *sorted(folder.rglob("*"))][:30]:
        try:
            status = path.lstat()
            found.append(f"{path.relative_to(folder)} {oct(status.st_mode & 0o7777)} {'dir' if path.is_dir() else 'file'}")
        except OSError as error:
            found.append(f"{path.relative_to(folder)} unreadable: {error.strerror}")
    if sys.platform != "win32":
        table = subprocess.run(["mount"], capture_output=True, text=True).stdout
        found += [line for line in table.splitlines() if str(folder) in line or f"/private{folder}" in line]
    return found


class Cell:
    def __init__(self, cozy: Path, out: Path) -> None:
        self.cozy_binary, self.out = cozy, out

    def cozy(self, *words: str, timeout: float = 600, cwd: Path | None = None) -> tuple[int | None, str, float]:
        """Exit (None: it didn't answer within `timeout`), its output (all its stdout, then its stderr's end), and how long it took."""
        if log := os.environ.get("ARGV_LOG"):
            with open(log, "a") as logged:
                logged.write("".join(f"{word}\x1f" for word in words) + "\x1e")
        began = time.monotonic()
        try:
            done = subprocess.run([str(self.cozy_binary), *words], cwd=cwd, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return None, "", round(time.monotonic() - began, 1)
        return done.returncode, done.stdout + "\n" + done.stderr[-800:], round(time.monotonic() - began, 1)

    def state(self, folder: Path) -> str:
        code, said, _ = self.cozy("--json", "vault", "--directory", str(folder), "status", timeout=ANSWER)
        if code is None:
            return "no answer"
        found = answer(said)
        return str(found.get("state")) if found else f"refused: {said.strip()[-200:]}"

    def up(self) -> bool:
        code, _, _ = self.cozy("--json", "status", timeout=ANSWER)
        return code in (0, 1)

    def made(self, name: str, idle: int) -> tuple[Path, str]:
        """An encrypted project, unlocked with `idle` (0: none), outside the job's output: its folder and view."""
        folder = Path(tempfile.mkdtemp(prefix=f"folder-gone-{name}-")) / "project"
        secret = self.out / "passphrase"
        code, said, _ = self.cozy("--json", "new", str(folder), "--passphrase-file", str(secret), "--recovery-out", str(self.out / f"{name}.recovery"))
        assert code == 0, f"cozy new {name}: {said}"
        code, said, _ = self.cozy("--json", "vault", "--directory", str(folder), "lock")
        assert code == 0, f"lock {name} before its unlock: {said}"
        code, said, _ = self.cozy("--json", "vault", "--directory", str(folder), "unlock", "--passphrase-file", str(secret), "--idle", str(idle))
        assert code == 0, f"unlock {name}: {said}"
        return folder, str(cast(dict[str, object], answer(said)["vault"])["view"])

    def removed(self, folder: Path) -> dict[str, object]:
        """The folder removed from outside, retried for 20 s as a person would: whether it went."""
        began = time.monotonic()
        while time.monotonic() - began < 20:
            shutil.rmtree(folder, ignore_errors=True)
            if not folder.exists():
                break
            time.sleep(0.5)
        return {"removed": not folder.exists(), "s": round(time.monotonic() - began, 1)}

    def put_away(self, folder: Path) -> None:
        """A folder that couldn't go (Linux, macOS): locked, then removed, its sealed underlay opened first."""
        self.cozy("--json", "vault", "--directory", str(folder), "lock", timeout=ANSWER)
        for path in [folder, *folder.rglob("*")]:
            try:
                os.chmod(path, 0o700)
            except OSError:
                pass
        shutil.rmtree(folder, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)
    (out / "passphrase").write_text("a passphrase long enough for this cell")
    cell = Cell(args.cozy, out)
    facts: dict[str, object] = {"os": sys.platform}
    failures: list[str] = []
    try:
        # 1. Its folder removed, then a lock asked of it.
        folder, view = cell.made("lock", 0)
        locked: dict[str, object] = {**cell.removed(folder), "mounted": mounted(view)}
        facts["lock"] = locked
        if locked["removed"]:
            code, said, seconds = cell.cozy("--json", "vault", "--directory", str(folder), "lock", timeout=ANSWER * 3)
            locked.update({"lock_exit": code, "lock_s": seconds, "lock_said": said[-300:], "state": cell.state(folder),
                           "mounted_after": mounted(view), "up": cell.up()})
            if code is None or seconds > ANSWER:
                failures.append(f"the lock asked of a project whose folder is gone didn't answer within {ANSWER} s ({seconds} s)")
            if mounted(view):
                failures.append(f"its view {view} is still mounted after the lock")
            if "folder is gone" not in said:
                failures.append(f"the lock asked of a project whose folder is gone didn't say so: {said.strip()[-300:]}")
        else:
            locked.update(ended(cell, folder, view))
            failures.append(f"lock: its folder couldn't be removed in 20 s: {locked}")
            cell.put_away(folder)

        # 2. Its folder removed, then its idle lock.
        folder, view = cell.made("idle", IDLE)
        idled: dict[str, object] = {**cell.removed(folder), "mounted": mounted(view)}
        facts["idle"] = idled
        if idled["removed"]:
            began = time.monotonic()
            state = cell.state(folder)
            while state in ("unlocked", "locking", "no answer") and time.monotonic() - began < IDLE + IDLE_BOUND:
                time.sleep(2)
                state = cell.state(folder)
            idled.update({"state": state, "ended_s": round(time.monotonic() - began, 1), "mounted_after": mounted(view), "up": cell.up()})
            if state in ("unlocked", "locking", "no answer"):
                failures.append(f"its session lasts {IDLE + IDLE_BOUND} s past its folder's removal with a {IDLE} s idle limit: {state}")
        else:
            idled.update(ended(cell, folder, view))
            failures.append(f"idle: its folder couldn't be removed in 20 s: {idled}")
            cell.put_away(folder)

        # 2b. Its folder moved away whole, as to the Trash: no view of it stays mounted, and it goes from there.
        folder, view = cell.made("moved", 0)
        trash = Path(tempfile.mkdtemp(prefix="folder-gone-trash-"))
        try:
            os.rename(folder, trash / "project")
            moved: dict[str, object] = {"moved": True}
        except OSError as error:
            moved = {"moved": False, "why": str(error)}
        facts["moved"] = moved
        if moved["moved"]:
            there = str(trash / "project" / Path(view).relative_to(folder)) if Path(view).is_relative_to(folder) else view
            began = time.monotonic()
            while (mounted(view) or mounted(there)) and time.monotonic() - began < MOVED_BOUND:
                time.sleep(0.5)
            moved.update({"unmounted_s": round(time.monotonic() - began, 1), "mounted": mounted(view) or mounted(there),
                          "state": cell.state(folder), **{f"then_{key}": value for key, value in cell.removed(trash / "project").items()}})
            if moved["mounted"]:
                failures.append(f"moved: its view stays mounted {MOVED_BOUND} s after its folder moved away: {there}")
            if moved["state"] == "unlocked":
                failures.append("moved: Calcifer still says the project is unlocked after its folder moved away")
            if not moved["then_removed"]:
                moved["left"] = left(trash / "project")
                failures.append(f"moved: its folder can't be removed from where it went: {moved}")
        else:
            cell.put_away(folder)

        # 2c. Its keybag alone deleted: the session holds the only copy of the keys, so it stays open (asked of, and on the clock).
        folder, view = cell.made("keys", 0)
        keybag = folder / "project.vault.keys.json"
        kept = keybag.read_bytes()
        keybag.unlink()
        code, said, _ = cell.cozy("--json", "vault", "--directory", str(folder), "unlock", "--passphrase-file", str(out / "passphrase"),
                                  timeout=ANSWER)
        time.sleep(15)
        keys: dict[str, object] = {"asked_exit": code, "asked_said": said.strip()[-200:], "mounted": mounted(view)}
        keybag.write_bytes(kept)
        keys["state"] = cell.state(folder)
        facts["keys"] = keys
        if keys["state"] != "unlocked" or not keys["mounted"]:
            failures.append(f"keys: its session ended when only its keybag was deleted: {keys}")
        code, said, _ = cell.cozy("--json", "vault", "--directory", str(folder), "lock", timeout=ANSWER)
        keys["locked_after"] = code == 0
        cell.put_away(folder)

        # 3. Calcifer answers, and a restart, which lets go of every view it holds, ends.
        facts["up"] = cell.up()
        if not facts["up"]:
            failures.append(f"Calcifer doesn't answer within {ANSWER} s after a project's folder was removed")
        code, said, seconds = cell.cozy("--json", "restart", timeout=RESTART * 2)
        facts["restart"] = {"exit": code, "s": seconds, "said": said[-300:], "up": cell.up()}
        if code is None or seconds > RESTART:
            failures.append(f"cozy restart didn't end within {RESTART} s after a project's folder was removed ({seconds} s)")
        elif code != 0:
            failures.append(f"cozy restart failed after a project's folder was removed: {said.strip()[-300:]}")
    finally:
        (out / "folder-gone.json").write_text(json.dumps(facts, indent=2, default=str))
    print(f"folder gone: {json.dumps(facts)}", flush=True)
    assert not failures, "folder gone: " + "; ".join(failures)


if __name__ == "__main__":
    main()
