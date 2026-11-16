"""cl-1049: which context stops securefs mounting on Windows, without Calcifer: one backing created, then mounted to a free drive
letter attached to this console, detached from any console (as cozy starts Calcifer), with no window, and in the environment
project-mount's supervisor clears to. Each reports its exit code, whether the drive appeared, and securefs's stderr (synthetic data
only). One JSON document on stdout.

  python e2e/cli/windows_vault_probe.py --securefs SECUREFS --out DIR
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import secrets
import string
import subprocess
import time

DETACHED_PROCESS = 0x00000008
CREATE_NO_WINDOW = 0x08000000
# project-mount's clean_command: what a supervisor and its securefs keep of the environment.
KEPT = ("PATH", "HOME", "SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR", "USERPROFILE")


def free_drive() -> str:
    for letter in reversed(string.ascii_uppercase[3:]):
        if not Path(f"{letter}:\\").exists():
            return f"{letter}:"
    raise SystemExit("no free drive letter")


def mount(securefs: Path, backing: Path, secret: str, label: str, flags: int, env: dict[str, str] | None, out: Path) -> dict[str, object]:
    drive = free_drive()
    errors = out / f"{label}.stderr"
    with errors.open("wb") as said:
        process = subprocess.Popen([str(securefs), "mount", str(backing), drive], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=said,
                                   creationflags=flags, env=env)
        assert process.stdin is not None
        process.stdin.write((secret + "\n").encode())
        process.stdin.close()
        appeared = False
        for _ in range(60):
            if Path(drive + "\\").exists():
                appeared = True
                break
            if process.poll() is not None:
                break
            time.sleep(0.25)
        code = process.poll()
        if code is None:
            subprocess.run([str(securefs), "unmount", drive], capture_output=True, timeout=30)
            try:
                code = process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                code = process.wait()
    return {"drive": drive, "appeared": appeared, "exit": code, "stderr": errors.read_text(errors="replace")[-1500:]}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--securefs", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    secret = secrets.token_urlsafe(32)[:43]
    backing = args.out / "backing"
    created = subprocess.run([str(args.securefs), "create", "--format", "lite", "--argon2-t", "1", "--argon2-m", "8", "--argon2-p", "1", str(backing)],
                             input=(secret + "\n" + secret + "\n").encode(), capture_output=True)
    report: dict[str, object] = {"version": subprocess.run([str(args.securefs), "version"], capture_output=True, text=True).stdout.strip()[:200],
                                 "create": {"exit": created.returncode, "stderr": created.stderr.decode(errors="replace")[-1500:]}}
    clean = {name: os.environ[name] for name in KEPT if name in os.environ} | {"LC_ALL": "C"}
    if created.returncode == 0:
        for label, flags, env in (("attached", 0, None), ("detached", DETACHED_PROCESS, None), ("no_window", CREATE_NO_WINDOW, None),
                                  ("clean_env", 0, clean), ("detached_clean_env", DETACHED_PROCESS, clean)):
            report[label] = mount(args.securefs, backing, secret, label, flags, env, args.out)
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
