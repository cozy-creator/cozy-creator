"""A Calcifer swap without downtime (cl-1113). Paul, 11-16: "it should be overlapping (v1 → v2) so I never have downtime."

`cozy restart` on a busy Calcifer, the way every update runs: a client asks `/v1/status` every 10 ms on fresh connections and another
on one kept alive, an event stream is open, and an encrypted project is unlocked. Not one request may fail, the stream must say
`restarting` and be back within 1 s, the address and bearer must stay, and the project must still be unlocked after. Then the
installed cozy is one that can't start: `cozy restart` must fail saying so, the running Calcifer must serve on throughout, and the
binary it ran must be back in place.

swap.py --cozy BIN --out DIR
"""

import argparse
import http.client
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import TypedDict, cast

HOME = Path.home() / ".cozy"
HOLDER = HOME / "calcifer" / "calcifer.lock.holder"
EXE = ".exe" if sys.platform == "win32" else ""


class Holder(TypedDict):
    address: str
    token: str
    pid: int
    version: str


def holder() -> Holder:
    return cast(Holder, json.loads(HOLDER.read_text()))


def place(source: Path, at: Path) -> None:
    """`source` at `at`, renamed into place as an update does: a running process keeps the inode it runs (a copy over it is refused)."""
    staged = at.with_name(at.name + ".staging")
    # Its bytes alone: a system binary's flags (macOS) don't copy.
    shutil.copyfile(source, staged)
    os.chmod(staged, 0o755)
    os.replace(staged, at)


def installed() -> Path:
    record = HOME / "installed"
    if record.is_file() and Path(record.read_text().strip()).is_file():
        return Path(record.read_text().strip())
    return HOME / "bin" / f"cozy{EXE}"


class Asking:
    """`GET /v1/status` every 10 ms: on a fresh connection each time, or on one kept alive, as pooled clients do (one that finds its
    kept connection closed before any answer asks again on a new one, as reqwest and browsers do; that is counted, not failed)."""

    def __init__(self, address: str, token: str, keep_alive: bool) -> None:
        self.host, port = address.rsplit(":", 1)
        self.port, self.token, self.keep_alive = int(port), token, keep_alive
        self.failures: list[str] = []
        self.asked = self.again = 0
        self.slowest = 0.0
        self.going = True
        self.thread = threading.Thread(target=self.run, daemon=True)

    def ask(self, connection: http.client.HTTPConnection) -> None:
        connection.request("GET", "/v1/status", headers={"Authorization": f"Bearer {self.token}"})
        response = connection.getresponse()
        response.read()
        if response.status != 200:
            raise RuntimeError(f"status {response.status}")

    def run(self) -> None:
        kept: http.client.HTTPConnection | None = None
        while self.going:
            began = time.monotonic()
            try:
                if self.keep_alive:
                    reused = kept is not None
                    kept = kept or http.client.HTTPConnection(self.host, self.port, timeout=30)
                    try:
                        self.ask(kept)
                    # A kept connection the server closed meanwhile, as each OS says it (Windows: WSAECONNABORTED).
                    except (http.client.RemoteDisconnected, ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
                        if not reused:
                            raise
                        self.again += 1
                        kept.close()
                        kept = http.client.HTTPConnection(self.host, self.port, timeout=30)
                        self.ask(kept)
                else:
                    fresh = http.client.HTTPConnection(self.host, self.port, timeout=30)
                    try:
                        self.ask(fresh)
                    finally:
                        fresh.close()
            except Exception as e:
                self.failures.append(f"{time.time():.3f} {type(e).__name__}: {e}")
                if kept is not None:
                    kept.close()
                    kept = None
            self.asked += 1
            self.slowest = max(self.slowest, time.monotonic() - began)
            time.sleep(0.01)

    def start(self) -> "Asking":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.going = False
        self.thread.join(timeout=60)


class Following:
    """`GET /v1/events`: its events, and how long it was without a stream after each end."""

    def __init__(self, address: str, token: str) -> None:
        self.host, port = address.rsplit(":", 1)
        self.port, self.token = int(port), token
        self.events: list[str] = []
        self.gaps: list[float] = []
        self.failures: list[str] = []
        self.going = True
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self) -> None:
        ended: float | None = None
        while self.going:
            try:
                connection = http.client.HTTPConnection(self.host, self.port, timeout=60)
                connection.request("GET", "/v1/events", headers={"Authorization": f"Bearer {self.token}"})
                response = connection.getresponse()
                if response.status != 200:
                    raise RuntimeError(f"status {response.status}")
                while self.going:
                    line = response.readline()
                    if not line:
                        break
                    text = line.decode(errors="replace").strip()
                    if not text.startswith("data: "):
                        continue
                    kind = str(json.loads(text[6:]).get("type"))
                    self.events.append(kind)
                    if kind == "hello" and ended is not None:
                        self.gaps.append(time.monotonic() - ended)
                        ended = None
                connection.close()
            except Exception as e:
                if self.going:
                    self.failures.append(f"{time.time():.3f} {type(e).__name__}: {e}")
            if ended is None:
                ended = time.monotonic()
            # Straight back after `restarting`, as the page does; otherwise as its backoff's first step.
            if not self.events or self.events[-1] != "restarting":
                time.sleep(0.25)

    def start(self) -> "Following":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.going = False


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cozy", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)
    results: dict[str, object] = {}

    def cozy(*words: str, binary: Path | None = None, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
        argv = [str(binary or args.cozy), *words]
        if log := os.environ.get("ARGV_LOG"):
            with open(log, "a") as logged:
                logged.write("".join(f"{word}\x1f" for word in words[:]) + "\x1e")
        return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=600)

    try:
        assert cozy("--json", "status").returncode in (0, 1), "cozy status didn't run"
        project = out / "sealed"
        shutil.rmtree(project, ignore_errors=True)
        (out / "passphrase").write_text("a passphrase long enough for this cell")
        made = cozy("--json", "new", str(project), "--passphrase-file", str(out / "passphrase"), "--recovery-out", str(out / "recovery"))
        assert made.returncode == 0, f"cozy new --passphrase-file: {made.stdout}{made.stderr}"

        def vault() -> str:
            said = cozy("--json", "vault", "status", cwd=project)
            return str(json.loads(said.stdout).get("state")) if said.returncode == 0 else f"failed: {said.stdout}{said.stderr}"

        if vault() != "unlocked":
            unlocked = cozy("--json", "vault", "unlock", "--passphrase-file", str(out / "passphrase"), cwd=project)
            assert unlocked.returncode == 0, f"unlock: {unlocked.stdout}{unlocked.stderr}"
        assert vault() == "unlocked", f"the encrypted project isn't unlocked: {vault()}"

        # 1. A swap onto the installed cozy, busy.
        before = holder()
        fresh = Asking(before["address"], before["token"], keep_alive=False).start()
        kept = Asking(before["address"], before["token"], keep_alive=True).start()
        stream = Following(before["address"], before["token"]).start()
        time.sleep(1)
        began = time.monotonic()
        restarted = cozy("--json", "restart")
        took = time.monotonic() - began
        time.sleep(2)
        for asking in (fresh, kept):
            asking.stop()
        stream.stop()
        after = holder()
        followed = {"events": [e for e in stream.events if e != "run.updated"][-40:], "gaps_s": [round(g, 3) for g in stream.gaps], "failures": stream.failures[:20]}
        state = vault()
        results["swap"] = {
            "restart": {"code": restarted.returncode, "stdout": restarted.stdout[-2000:], "stderr": restarted.stderr[-2000:], "s": round(took, 3)},
            "before": {"pid": before["pid"], "version": before["version"], "address": before["address"]},
            "after": {"pid": after["pid"], "version": after["version"], "address": after["address"]},
            "fresh": {"asked": fresh.asked, "failures": fresh.failures[:20], "slowest_s": round(fresh.slowest, 3)},
            "kept": {"asked": kept.asked, "again": kept.again, "failures": kept.failures[:20], "slowest_s": round(kept.slowest, 3)},
            "stream": followed,
            "vault": state,
        }
        assert restarted.returncode == 0, f"cozy restart failed: {restarted.stdout}{restarted.stderr}"
        assert after["pid"] != before["pid"], "the holder record still names the Calcifer before the restart"
        assert after["address"] == before["address"] and after["token"] == before["token"], "the address or the bearer changed"
        assert not fresh.failures, f"requests on fresh connections failed: {fresh.failures[:5]}"
        assert not kept.failures, f"requests on a kept connection failed: {kept.failures[:5]}"
        assert "restarting" in stream.events, f"the stream never said restarting: {followed}"
        assert stream.gaps and max(stream.gaps) <= 1.0, f"the stream was gone longer than 1 s: {followed}"
        assert state == "unlocked", f"the encrypted project didn't stay unlocked: {state}"
        status = cozy("--json", "status", cwd=project)
        assert status.returncode == 0, f"the encrypted project isn't readable after the swap: {status.stdout}{status.stderr}"
        # The view the next generation took locks as any does: its mount ends.
        locked = cozy("--json", "vault", "lock", cwd=project)
        results["lock_after"] = {"code": locked.returncode, "said": (locked.stdout + locked.stderr)[-1500:], "state": vault()}
        assert locked.returncode == 0 and vault() == "locked", f"the carried view didn't lock: {results['lock_after']}"

        # 2. An installed cozy that can't start: the running one serves on, its binary put back.
        target = installed()
        good = out / f"cozy-good{EXE}"
        shutil.copy2(target, good)
        previous = target.with_name(".cozy-previous")
        place(target, previous)
        broken = Path(os.environ.get("SystemRoot", "C:\\Windows")) / "System32" / "findstr.exe" if EXE else Path(shutil.which("false") or "/bin/false")
        place(broken, target)
        before = holder()
        fresh = Asking(before["address"], before["token"], keep_alive=False).start()
        try:
            failed = cozy("--json", "restart", binary=good)
        finally:
            fresh.stop()
            put_back = target.read_bytes() == good.read_bytes()
            if not put_back:
                os.replace(previous if previous.is_file() else good, target)
        after = holder()
        results["broken"] = {
            "restart": {"code": failed.returncode, "stdout": failed.stdout[-2000:], "stderr": failed.stderr[-2000:]},
            "pid_kept": after["pid"] == before["pid"],
            "put_back": put_back,
            "previous_left": previous.is_file(),
            "fresh": {"asked": fresh.asked, "failures": fresh.failures[:20]},
        }
        assert failed.returncode != 0, "cozy restart onto a cozy that can't start succeeded"
        assert "stays as it is" in failed.stdout + failed.stderr, f"cozy restart didn't say Calcifer stays: {failed.stdout}{failed.stderr}"
        assert after["pid"] == before["pid"], "the running Calcifer didn't serve on"
        assert put_back, "the binary the running Calcifer ran wasn't put back"
        assert not fresh.failures, f"requests failed while the next one failed to start: {fresh.failures[:5]}"
        results["ok"] = True
    except AssertionError as e:
        results["ok"] = False
        results["failed"] = str(e)
        print(f"swap: {e}", file=sys.stderr)
    finally:
        (out / "results.json").write_text(json.dumps(results, indent=2))
    if not results.get("ok"):
        sys.exit(1)


if __name__ == "__main__":
    main()
