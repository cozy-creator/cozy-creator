# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=11", "python-snappy>=0.7"]
# ///
"""Paul's invariant on any OS (cl-984): an encrypted project made, run with a marker prompt and locked leaves nothing of itself
outside its folder: not its id or name (but in the folder's path), its prompt, source, run or output. The portable half of
containment's lock scans, for macOS and Windows runners: the installed CLI only, no browser, no root.

  uv run --no-project e2e/cli/containment/notraces.py --cli COZY --root DIR --out DIR
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
from typing import cast

from common import Failure, require, sha
from scan import Scanner

PROGRAM = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
class Request(msgspec.Struct, forbid_unknown_fields=True):
    prompt: str
class Answer(msgspec.Struct):
    echoed: str
    report: File
@app.entrypoint(accelerator=False)
def echo(payload: Request, out: Outputs) -> Answer:
    report = out.temporary_file(".txt")
    report.write_text(payload.prompt)
    return Answer(payload.prompt, out.save_file(report, media_type="text/plain"))
'''

Json = dict[str, object]


def documents(body: bytes) -> list[Json]:
    found: list[Json] = []
    for line in body.splitlines():
        try:
            value = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        if isinstance(value, dict):
            found.append(value)
    return found


def cozy(cli: Path, *args: str, cwd: Path | None = None, stdin: bytes | None = None) -> list[Json]:
    done = subprocess.run([str(cli), "--json", *args], cwd=cwd, input=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    answered = documents(done.stdout)
    if done.returncode != 0:
        codes = [error.get("code") for document in answered for error in [document.get("error")] if isinstance(error, dict)]
        raise Failure(f"cozy {args[0]} exited {done.returncode}: {codes}")
    return answered


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    scanner = Scanner()
    folder = args.root.resolve() / ("notraces-" + secrets.token_hex(12))
    credential = "proof-" + secrets.token_hex(32)
    scanner.add(credential)
    created = cozy(args.cli, "new", str(folder), "--passphrase-file", "-", stdin=(credential + "\n").encode())[-1]
    project = created.get("id")
    require(isinstance(project, str), "notraces_project_identity")
    project = cast(str, project)
    recovery = created.get("recovery_secret")
    if isinstance(recovery, str):
        scanner.add(recovery)
    vault = cozy(args.cli, "vault", "status", cwd=folder)[-1]
    require(vault.get("encrypted") is True and vault.get("state") == "unlocked" and isinstance(vault.get("view"), str), "notraces_born_encrypted")
    view = Path(cast(str, vault["view"]))
    prompt = "PROMPT_" + secrets.token_hex(24)
    scanner.add(prompt)
    (view / "probe.py").write_text(PROGRAM)
    scanner.add(sha(PROGRAM.encode())); scanner.add("sha256:" + sha(PROGRAM.encode()))
    # The prompt travels as a project-owned input file, never argv or environment.
    (view / "input.json").write_text(json.dumps({"prompt": prompt}))
    events = cozy(args.cli, "run", "probe.py/echo", "--input", "input.json", "--on", "local", cwd=folder)
    runs = [run for event in events for run in [event.get("run", event)] if isinstance(run, dict)]
    results = [run for run in runs if isinstance(run.get("result"), dict) and cast(Json, run["result"]).get("echoed") == prompt]
    require(bool(results), "notraces_run_result")
    for run in runs:
        if isinstance(run.get("id"), str):
            scanner.add(cast(str, run["id"]))
    scanner.add(sha(prompt.encode())); scanner.add("sha256:" + sha(prompt.encode()))
    locked = cozy(args.cli, "vault", "lock", cwd=folder)[-1]
    require(cast(Json, locked.get("vault", {})).get("state") == "locked", "notraces_locked")
    # Locked, only the folder's path may stay outside it: its id and name nowhere else.
    scanner.identity(folder, project, folder.name)
    home = Path.home()
    # macOS keeps a user's caches (QuickLook's thumbnails among them) beside its temporary folder: T's parent holds C too.
    temporary = Path(tempfile.gettempdir())
    roots = {home / ".cozy", home / ".cache", home / "Library" / "Caches", home / "AppData" / "Local", temporary,
             temporary.parent if sys.platform == "darwin" else temporary,
             Path("/tmp"), Path("/var/tmp"), Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())), args.root}
    result = scanner.scan(sorted({root.resolve() for root in roots if root.exists()}), [], args.out.resolve() / "notraces.json")
    counts = {key: value for key, value in result.items() if type(value) is int}
    (args.out / "notraces.json").write_text(json.dumps({"os": sys.platform, **counts}, indent=2))
    require(counts.get("files", 0) > 0, "notraces_scanned_nothing")
    require(counts.get("matches", 0) == 0, "outside_project_canary")
    print(json.dumps({"no_traces": True, "os": sys.platform, "files": counts.get("files"), "matches": 0}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Failure as failure:
        print(json.dumps({"no_traces": False, "failure": str(failure)}))
        sys.exit(1)
