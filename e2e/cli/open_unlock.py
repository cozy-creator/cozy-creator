"""`cozy open` in a locked encrypted project (cl-1002). At a terminal it asks for the passphrase or the recovery secret, unlocks,
and opens: the page's URL printed, through Cozy's private browser or, with no display, as an address any browser opens. `--json`
and a pipe never ask: they refuse `project_locked`, naming `cozy vault unlock`. `cozy status` at a terminal unlocks as open does
(cl-1066): every verb shares that path.

open_unlock.py --cozy BIN --project DIR --passphrase FILE --recovery FILE --out DIR
"""

import argparse
import errno
import fcntl
import json
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

PROMPT = b"Project passphrase or recovery secret: "


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", required=True)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--passphrase", type=Path, required=True)
    parser.add_argument("--recovery", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    project = args.project.resolve()
    passphrase = args.passphrase.read_text().strip()
    recovery = args.recovery.read_text().strip()

    # Any browser open goes here, not to the runner's: `cozy open` never starts the default browser.
    browser_bin = args.out / "browser-bin"
    browser_bin.mkdir(exist_ok=True)
    browser_log = args.out / "browser-opened.txt"
    for name in ("xdg-open", "open"):
        (browser_bin / name).write_text('#!/bin/sh\nprintf \'%s\\n\' "$1" >> "$COZY_TEST_BROWSER_LOG"\n')
        (browser_bin / name).chmod(0o755)
    env = dict(os.environ, PATH=f"{browser_bin}{os.pathsep}{os.environ.get('PATH', '')}", COZY_TEST_BROWSER_LOG=str(browser_log))
    receipt: dict[str, object] = {}

    def logged(words: list[str]) -> None:
        if log := os.environ.get("ARGV_LOG"):
            with open(log, "a", encoding="utf-8") as stream:
                stream.write("\x1f".join(words) + "\x1f\x1e")

    def cozy(*words: str) -> subprocess.CompletedProcess[str]:
        logged(list(words))
        return subprocess.run([args.cozy, *words], cwd=project, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300)

    def state() -> str:
        return str(json.loads(cozy("--json", "vault", "status").stdout)["state"])

    def lock() -> None:
        cozy("--json", "vault", "lock")
        for _ in range(120):
            if state() == "locked":
                return
            time.sleep(0.5)
        raise AssertionError("the project did not lock")

    def on_terminal(words: list[str], answers: list[str]) -> tuple[int, str, str, int]:
        """`cozy <words>` with its stdin and stderr a real terminal, its stdout a pipe; each prompt answered once echo is off."""
        logged(words)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 120, 0, 0))

        def controlling() -> None:
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        process = subprocess.Popen([args.cozy, *words], cwd=project, env=env, stdin=slave, stdout=subprocess.PIPE, stderr=slave,
                                   preexec_fn=controlling)
        os.close(slave)
        screen, entered, deadline = bytearray(), 0, time.monotonic() + 300
        try:
            while True:
                if time.monotonic() > deadline:
                    raise AssertionError(f"cozy {' '.join(words)} did not finish in 300 s: {screen.decode(errors='replace')[-600:]}")
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        block = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        block = b""
                    screen.extend(block)
                    if not block and process.poll() is not None:
                        break
                elif process.poll() is not None:
                    break
                if screen.count(PROMPT) > entered and not termios.tcgetattr(master)[3] & termios.ECHO:
                    if entered >= len(answers):
                        raise AssertionError(f"cozy {' '.join(words)} asked again: {screen.decode(errors='replace')[-600:]}")
                    os.write(master, answers[entered].encode() + b"\n")
                    entered += 1
            stdout = process.stdout.read().decode() if process.stdout else ""
            return process.wait(), stdout, screen.decode(errors="replace"), screen.count(PROMPT)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)

    def opened(name: str, code: int, stdout: str, stderr: str) -> None:
        assert code == 0, f"{name}: exit {code}: {stdout}{stderr[-600:]}"
        assert state() == "unlocked", f"{name}: the project is not unlocked after open"
        found = re.search(r"url:\s*\"?([^\"\s]+)", stdout)
        assert found, f"{name}: no URL printed: {stdout}"
        here = json.loads(cozy("--json", "--full", "status").stdout)["project"]["id"]
        assert re.fullmatch(rf"http://(127\.0\.0\.1|localhost|\[::1\]):\d+/projects/{re.escape(here)}", found.group(1)), f"{name}: {found.group(1)}"
        if "private: true" not in stdout:
            assert "isn't private" in stderr, f"{name}: no private browser, and it didn't say the address isn't private: {stderr[-600:]}"
        assert not browser_log.exists(), f"{name}: open started the default browser"
        receipt[name] = {"url": found.group(1), "private": "private: true" in stdout}

    if state() != "locked":
        lock()

    piped = cozy("open")
    assert piped.returncode != 0 and "cozy vault unlock" in piped.stdout + piped.stderr, f"open from a pipe: {piped.stdout}{piped.stderr}"
    assert "passphrase" not in (piped.stdout + piped.stderr).lower(), "open from a pipe asked for a credential"
    as_json = cozy("--json", "open")
    error = json.loads(as_json.stdout)["error"]
    assert as_json.returncode != 0 and error["code"] == "project_locked" and "cozy vault unlock" in json.dumps(error), f"--json open: {as_json.stdout}"
    code, stdout, stderr, asked = on_terminal(["--json", "open"], [])
    assert code != 0 and asked == 0 and json.loads(stdout)["error"]["code"] == "project_locked", f"--json open at a terminal: {stdout}{stderr[-600:]}"
    assert state() == "locked", "a refused open unlocked the project"
    receipt["refused"] = {"pipe": True, "json": True, "json_terminal": True}

    code, stdout, stderr, asked = on_terminal(["open"], ["deliberately wrong credential", passphrase])
    assert asked == 2 and "did not unlock" in stderr, f"open: a wrong passphrase was not asked again: {stderr[-600:]}"
    opened("passphrase", code, stdout, stderr)

    lock()
    code, stdout, stderr, asked = on_terminal(["open"], [recovery])
    assert asked == 1, f"open by recovery secret: asked {asked} times: {stderr[-600:]}"
    opened("recovery", code, stdout, stderr)

    # Every verb, not only `open`, unlocks a locked project at a terminal and goes on (cl-1066): `cozy status` asks once, says the
    # project, and leaves it unlocked. From a pipe it still refuses, asking nothing.
    lock()
    piped = cozy("status")
    assert piped.returncode != 0 and "cozy vault unlock" in piped.stdout + piped.stderr, f"status from a pipe: {piped.stdout}{piped.stderr}"
    assert "passphrase" not in (piped.stdout + piped.stderr).lower(), "status from a pipe asked for a credential"
    code, stdout, stderr, asked = on_terminal(["status"], [passphrase])
    assert code == 0 and asked == 1, f"status at a terminal: exit {code}, asked {asked}: {stdout}{stderr[-600:]}"
    assert state() == "unlocked", "status at a terminal left the project locked"
    receipt["status"] = {"asked": asked}

    (args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(json.dumps(receipt))


if __name__ == "__main__":
    try:
        main()
    except AssertionError as failed:
        print(f"open unlock: {failed}", file=sys.stderr)
        sys.exit(1)
