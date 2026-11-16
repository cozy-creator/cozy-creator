"""An encrypted project on this runner's OS through the installed CLI (cl-1049): made with a passphrase file and a recovery file,
written in its view, locked, unlocked by passphrase and by recovery secret, and read back each time. Portable (Linux, macOS,
Windows), standard library only; synthetic data. Ends with one JSON line; a failure exits 1 with a step code and the CLI's own
typed error.

  python3 e2e/cli/vault_native.py --cli COZY --root DIR
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

MARKER = "SYNTHETIC_NATIVE_VAULT_CONTENT_1763164800"


class Stop(Exception):
    pass


def cozy(cli: Path, *args: str, cwd: Path | None = None) -> dict[str, object]:
    done = subprocess.run([str(cli), "--json", *args], cwd=cwd, capture_output=True, text=True, errors="replace")
    answer: dict[str, object] = {}
    for line in done.stdout.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict):
            answer = value
    if done.returncode != 0:
        error = answer.get("error")
        said = json.dumps({k: error.get(k) for k in ("code", "message")}) if isinstance(error, dict) else done.stderr.strip()[-600:]
        raise Stop(f"cozy {' '.join(a for a in args if not os.path.isabs(a))} exited {done.returncode}: {said}")
    return answer


def status(cli: Path, project: Path) -> dict[str, object]:
    return cozy(cli, "vault", "status", cwd=project)


def view_of(state: dict[str, object]) -> Path:
    vault = state.get("vault", state)
    view = vault.get("view") if isinstance(vault, dict) else None
    if not isinstance(view, str) or not Path(view).is_dir():
        raise Stop(f"no readable view: {vault}")
    return Path(view)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    cli, root = args.cli, args.root.resolve()
    secrets = root / "credentials"
    secrets.mkdir(parents=True, exist_ok=True)
    passphrase, recovery = secrets / "passphrase", secrets / "recovery"
    passphrase.write_text("synthetic native vault passphrase\n")
    project = root / "born"
    steps: list[str] = []
    try:
        cozy(cli, "new", str(project), "--passphrase-file", str(passphrase), "--recovery-out", str(recovery))
        steps.append("created")
        state = status(cli, project)
        if not (state.get("encrypted") is True and state.get("state") == "unlocked"):
            raise Stop(f"new made no unlocked encrypted project: {state}")
        if not recovery.is_file() or not recovery.read_text().strip():
            raise Stop("new --recovery-out saved no recovery secret")
        view = view_of(state)
        (view / "files").mkdir(exist_ok=True)
        (view / "files" / "sentinel.txt").write_text(MARKER)
        steps.append("written")
        for label, unlock in (("passphrase", ["--passphrase-file", str(passphrase)]), ("recovery", ["--recovery-file", str(recovery)])):
            locked = cozy(cli, "vault", "lock", cwd=project)
            vault = locked.get("vault")
            if not (isinstance(vault, dict) and vault.get("state") == "locked"):
                raise Stop(f"lock did not answer locked: {locked}")
            if (view / "files" / "sentinel.txt").exists():
                raise Stop("locked, its old view still reads")
            for path in project.rglob("*"):
                if path.is_file() and MARKER.encode() in path.read_bytes():
                    raise Stop(f"locked, its backing holds the plaintext: {path.relative_to(project)}")
            steps.append(f"locked before {label}")
            view = view_of(cozy(cli, "vault", "unlock", *unlock, "--idle", "0", cwd=project))
            if (view / "files" / "sentinel.txt").read_text() != MARKER:
                raise Stop(f"unlocked by {label}, the file didn't read back")
            steps.append(f"read back by {label}")
        cozy(cli, "vault", "lock", cwd=project)
        steps.append("locked")
    except Stop as stop:
        print(json.dumps({"passed": False, "steps": steps, "stopped": str(stop)}))
        sys.exit(1)
    print(json.dumps({"passed": True, "steps": steps}))


if __name__ == "__main__":
    main()
