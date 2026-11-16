# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=11", "python-snappy>=0.7"]
# ///
"""Paul's invariant on any OS (cl-984): an encrypted project made, given a box with a marker prompt and an import, its thumbnail
drawn and (where this computer runs tools) a run of that prompt, then locked, leaves nothing of itself outside its folder: not its
id or name (but in the folder's path), its prompt, its import, source, run or output. The portable half of containment's lock
scans, for macOS and Windows runners: the installed CLI and Calcifer's own API, no browser, no root.

  uv run --no-project e2e/cli/containment/notraces.py --cli COZY --root DIR --out DIR
"""
from __future__ import annotations

import argparse
from collections import Counter
from http.client import HTTPConnection
import io
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time
from typing import cast
from urllib.parse import quote, urlsplit

from common import Failure, sha
from scan import Scanner, explained, places

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


class Stop(Exception):
    """Why the cell stopped: a fixed step code, and a CLI's typed error codes; never a body."""


def require(ok: object, code: str) -> None:
    if not ok:
        raise Stop(code)


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
        raise Stop(f"cozy {args[0]} exited {done.returncode}: {codes}")
    return answered


def calcifer(method: str, path: str, body: bytes | Json | None = None) -> tuple[int, bytes]:
    """Calcifer's API as its CLI asks it: its holder's loopback address and token."""
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_bytes())
    endpoint = urlsplit("http://" + holder["address"])
    require(endpoint.hostname in {"127.0.0.1", "::1", "localhost"} and endpoint.port is not None, "holder_loopback")
    headers = {"Authorization": "Bearer " + holder["token"], "cozy-project-storage": "3", "Cozy-Client": "cli"}
    data = None
    if isinstance(body, dict):
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    elif body is not None:
        data, headers["Content-Type"] = body, "application/octet-stream"
    if method != "GET":
        headers["Idempotency-Key"] = "notraces-" + secrets.token_hex(16)
    connection = HTTPConnection(cast(str, endpoint.hostname), endpoint.port, timeout=60)
    try:
        connection.request(method, path, body=data, headers=headers)
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def picture(marker: str) -> bytes:
    from PIL import Image, PngImagePlugin
    image = Image.new("RGB", (64, 48), (secrets.randbelow(256), secrets.randbelow(256), secrets.randbelow(256)))
    metadata = PngImagePlugin.PngInfo()
    metadata.add_text("Description", marker)
    out = io.BytesIO()
    image.save(out, format="PNG", pnginfo=metadata)
    return out.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    scanner = Scanner()
    credential = "proof-" + secrets.token_hex(32)
    scanner.add(credential)
    folder = args.root.resolve() / ("notraces-" + secrets.token_hex(12))
    try:
        created = cozy(args.cli, "new", str(folder), "--passphrase-file", "-", stdin=(credential + "\n").encode())[-1]
    except Stop as stopped:
        # A runner with no encrypted filesystem provider (Windows CI installs no WinFsp): `cozy doctor --fix` installs it, as an
        # administrator's would, whatever it exits; then once more, in a new folder, and a second refusal fails the cell.
        if "'vault_provider'" not in str(stopped) and "'vault_dependency_missing'" not in str(stopped):
            raise
        subprocess.run([str(args.cli), "doctor", "--fix"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        folder = args.root.resolve() / ("notraces-" + secrets.token_hex(12))
        created = cozy(args.cli, "new", str(folder), "--passphrase-file", "-", stdin=(credential + "\n").encode())[-1]
    require(isinstance(created.get("id"), str), "notraces_project_identity")
    project = cast(str, created["id"])
    recovery = created.get("recovery_secret")
    if isinstance(recovery, str):
        scanner.add(recovery)
    vault = cozy(args.cli, "vault", "status", cwd=folder)[-1]
    require(vault.get("encrypted") is True and vault.get("state") == "unlocked" and isinstance(vault.get("view"), str), "notraces_born_encrypted")
    view = Path(cast(str, vault["view"]))
    base = "/v1/projects/" + quote(project, safe="")
    # A box whose words are the prompt, as the page makes one.
    prompt, box = "PROMPT_" + secrets.token_hex(24), "box_" + secrets.token_hex(8)
    scanner.add(prompt); scanner.add(box)
    status, _ = calcifer("POST", base + "/assets", {"name": box, "exact": True, "fields": {"text": prompt}, "at": {"x": 0, "y": 0, "w": 320}})
    require(200 <= status < 300, f"notraces_box: HTTP {status}")
    # An import, and its thumbnail drawn, as the page asks one.
    marker, imported = "IMPORT_" + secrets.token_hex(24), "pic_" + secrets.token_hex(8)
    scanner.add(marker); scanner.add(imported)
    image = picture(marker)
    digest = "sha256:" + sha(image)
    scanner.add(digest); scanner.add(digest.removeprefix("sha256:"))
    status, _ = calcifer("PUT", "/v1/project-files/" + digest + "?project=" + quote(project, safe=""), image)
    require(200 <= status < 300, f"notraces_upload: HTTP {status}")
    status, _ = calcifer("POST", base + "/imports", {"source": digest, "name": imported, "exact": True, "at": {"x": 400, "y": 0, "w": 320}})
    require(200 <= status < 300, f"notraces_import: HTTP {status}")
    for _ in range(120):
        status, _ = calcifer("GET", base + "/thumbs/" + digest.removeprefix("sha256:") + "?w=128")
        if status != 202:
            break
        time.sleep(0.5)
    require(status == 200, f"notraces_thumbnail: HTTP {status}")
    # Where this computer runs tools, a run of the prompt too, from the project's own input file (never argv or environment).
    status, said = calcifer("GET", "/v1/status")
    require(status == 200, "notraces_status")
    local = json.loads(said).get("local_usable") is True
    if local:
        (view / "probe.py").write_text(PROGRAM)
        scanner.add(sha(PROGRAM.encode())); scanner.add("sha256:" + sha(PROGRAM.encode()))
        (view / "input.json").write_text(json.dumps({"prompt": prompt}))
        events = cozy(args.cli, "run", "probe.py/echo", "--input", "input.json", "--on", "local", cwd=folder)
        runs = [run for event in events for run in [event.get("run", event)] if isinstance(run, dict)]
        require(any(isinstance(run.get("result"), dict) and cast(Json, run["result"]).get("echoed") == prompt for run in runs), "notraces_run_result")
        for run in runs:
            if isinstance(run.get("id"), str):
                scanner.add(cast(str, run["id"]))
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
    located = places(home, Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())), projects=args.root.resolve(), temporary=temporary.resolve())
    result = scanner.scan(sorted({root.resolve() for root in roots if root.exists()}), [], args.out.resolve() / "notraces.json", located)
    counts = {key: value for key, value in result.items() if type(value) is int}
    # Every path it couldn't read, named: on Windows each must be one WINDOWS_UNREADABLE explains, or the cell fails.
    unreadable = sorted(set(scanner.unreadable))
    (args.out / "notraces.json").write_text(json.dumps({"os": sys.platform, "ran": local, **counts, "unreadable_paths": unreadable}, indent=2))
    for path in unreadable:
        print("unreadable: " + path, file=sys.stderr)
    require(counts.get("files", 0) > 0, "notraces_scanned_nothing")
    require(counts.get("matches", 0) == 0, "outside_project_canary: " + json.dumps({key: value for key, value in counts.items() if key.startswith("match")}))
    cases = Counter(explained(path) for path in unreadable) if sys.platform == "win32" else Counter[str | None]()
    unknown = [path for path in unreadable if explained(path) is None] if sys.platform == "win32" else []
    require(not unknown, "unreadable_unexplained: " + json.dumps(unknown))
    print(json.dumps({"no_traces": True, "os": sys.platform, "ran": local, "files": counts.get("files"), "matches": 0,
                      "unreadable": len(unreadable), "unreadable_cases": {str(name): count for name, count in sorted(cases.items(), key=str)}}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (Stop, Failure) as failure:
        print(json.dumps({"no_traces": False, "failure": str(failure)}))
        sys.exit(1)
