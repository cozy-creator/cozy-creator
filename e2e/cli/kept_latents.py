#!/usr/bin/env python3
"""kept_latents (cl-1046): a local run's latents are an ordinary file of its take, in its project. Coordinator (11-16): "the latent is
an ordinary file of its take inside the project, on the one export path, and purge already handles its lifetime"; Paul: a project's
files live in its folder.

In a plain project and an encrypted one, boxes of project/kept.py's `shot` render on local, with small latents and with large ones
(both renders timed, and how long the large latents take to land); the latents are in the project's files/. Then the project's machine
store is aged past its TTL and grace, the local machine restarts and a render writes to that store, so its upkeep collects what nothing
holds: a run naming the take's latents by their hash still reads them whole, from files/. Deleted, a take nothing read loses its latents
file from files/ when its undo window ends (Calcifer runs with a short one here). That take's latents are large, so they land after its
run has arrived (cl-1093): they're its version's file, with no loose run row of the project's to keep them past its purge."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent))
from project_http import project_path, request  # noqa: E402

KEPT = Path(__file__).resolve().parent / "project" / "kept.py"
LARGE = 256 << 20
WEEK = 8 * 24 * 3600


def cozy(cli: Path, *words: str, cwd: Path | None = None, out: Path | None = None, name: str = "") -> subprocess.CompletedProcess[bytes]:
    done = subprocess.run([str(cli), *words], cwd=cwd, capture_output=True, check=False)
    if out is not None and name:
        (out / f"{name}.out").write_bytes(done.stdout + b"\n--- stderr\n" + done.stderr)
    return done


def api(project: Path, suffix: str, method: str = "GET", body: dict[str, object] | None = None) -> dict[str, object]:
    return cast(dict[str, object], json.loads(request(project_path(project, suffix), method, body)))  # cast: Calcifer's JSON


def take_files(project: Path, box: str) -> dict[str, object]:
    versions = api(project, f"/assets/{box}")["versions"]
    assert versions, f"{box} has no take"
    return versions[-1]["files"]  # type: ignore[index,no-any-return]


def digest(file: object) -> str:
    sha = file.get("sha256") if isinstance(file, dict) else file
    assert isinstance(sha, str) and sha, f"no digest in {file!r}"
    return sha.removeprefix("sha256:")


def held(folder: Path, hex_digest: str) -> list[Path]:
    """Where `folder` has the bytes named `hex_digest`."""
    return [path for path in folder.rglob(f"{hex_digest}*") if path.is_file()] if folder.is_dir() else []


def render(cli: Path, project: Path, out: Path, box: str, *extra: str) -> float:
    began = time.monotonic()
    done = cozy(cli, "--json", "render", box, *extra, "--on", "local", cwd=project, out=out, name=f"render-{project.name}-{box}")
    assert done.returncode == 0, f"render {box} in {project.name}: {done.stderr[-400:]!r}"
    return time.monotonic() - began


def restart_machine(cli: Path) -> None:
    """The local machine stopped (its process killed, as lib.sh's machine_stop) and started again: nothing it held in memory stays."""
    holders = list((Path.home() / ".cozy/machine").rglob("agent.lock.holder"))
    assert holders, "no local machine to restart"
    pid = json.loads(holders[0].read_text())["pid"]
    os.kill(pid, signal.SIGTERM)
    for _ in range(240):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            break
        time.sleep(0.5)
    for _ in range(120):
        shown = cozy(cli, "--json", "machine", "show", "local")
        if shown.returncode == 0 and json.loads(shown.stdout or b"{}").get("state") == "ready":
            return
        time.sleep(5)
    raise AssertionError("the local machine did not start again")


def loose_rows(view: Path, hex_digest: str) -> int:
    """How many of the project's run rows name the bytes `hex_digest`: a run's output no version holds, which a purge keeps."""
    db = sqlite3.connect(f"file:{view / 'project.db'}?mode=ro", uri=True, timeout=30)
    try:
        return int(db.execute("SELECT count(*) FROM run_files WHERE sha256 = ?", (f"sha256:{hex_digest}",)).fetchone()[0])
    finally:
        db.close()


def landed(folder: Path, hex_digest: str, began: float, seconds: float = 60) -> float | None:
    """Seconds from `began` until `folder` has the bytes named `hex_digest`; none if they don't come within `seconds`."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if held(folder, hex_digest):
            return round(time.monotonic() - began, 2)
        time.sleep(0.2)
    return None


def project_case(cli: Path, out: Path, project: Path, view: Path) -> dict[str, object]:
    facts: dict[str, object] = {}
    # A box per render, each its own request, so each runs: the first installs its environment, the next are measured. `gone` is
    # the take deleted at the end, which nothing else reads: its latents large, so they land after its run arrives (cl-1093).
    for box, size in (("warm", 64), ("small", 65), ("large", LARGE), ("gone", LARGE + 1), ("after", 66)):
        api(project, "/assets", "POST", {"name": box, "tool": f"{KEPT}/shot", "kind": "video", "exact": True, "fields": {"size": size}})
    render(cli, project, out, "warm")
    facts["small_s"] = round(render(cli, project, out, "small"), 2)
    began = time.monotonic()
    facts["large_s"] = round(render(cli, project, out, "large"), 2)
    latents = digest(take_files(project, "large")["latents"])
    # The render answers once its take is; its latents land in files/ as its other outputs do, and this says when.
    facts["large_latents_landed_s"] = landed(view / "files", latents, began)
    facts["latents_in_files"] = [str(path) for path in held(view / "files", latents)]
    render(cli, project, out, "gone")
    doomed = digest(take_files(project, "gone")["latents"])
    facts["gone_latents_landed"] = landed(view / "files", doomed, time.monotonic()) is not None
    store = view / ".vault-machine" / "objects"

    # Aged past the TTL and grace, the machine restarted, and its upkeep run by a render that writes to the store.
    old = time.time() - WEEK
    for path in store.rglob("*"):
        try:
            os.utime(path, (old, old), follow_symlinks=False)
        except OSError:
            pass
    restart_machine(cli)
    render(cli, project, out, "after")
    facts["store_copies_after_upkeep"] = [str(path) for path in held(store, latents)]

    # A run of the project naming the take's latents by their hash, as a box using them names them: read whole.
    read = cozy(cli, "--json", "run", f"{KEPT}/read", f"data=sha256:{latents}", "--on", "local", cwd=project, out=out, name=f"read-{project.name}")
    facts["read_latents"] = read.returncode == 0 and f"sha256:{latents}".encode() in read.stdout
    if not facts["read_latents"]:
        facts["read_failed"] = (read.stdout[-300:] + read.stderr[-300:]).decode(errors="replace")

    # Landed after its run arrived, the take's latents are its version's file, no loose run row (cl-1093).
    facts["gone_loose_rows"] = loose_rows(view, doomed)

    # Deleted: a take nothing else read; its latents file leaves files/ once its undo window ends.
    deleted = cozy(cli, "--json", "delete", "gone", cwd=project, out=out, name=f"delete-{project.name}")
    facts["deleted"] = deleted.returncode == 0
    for _ in range(90):
        if not held(view / "files", doomed):
            break
        time.sleep(2)
    facts["latents_after_delete"] = [str(path) for path in held(view / "files", doomed)]
    return facts


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--passphrase", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    facts: dict[str, object] = {}

    plain = args.work / "plain"
    assert cozy(args.cli, "--json", "new", "--unencrypted", str(plain)).returncode == 0, "cozy new --unencrypted failed"
    facts["plain"] = project_case(args.cli, args.out, plain, plain)

    encrypted = args.work / "encrypted"
    made = cozy(args.cli, "--json", "new", str(encrypted), "--passphrase-file", str(args.passphrase), "--recovery-out", str(args.out / "recovery"))
    assert made.returncode == 0, f"cozy new --passphrase-file failed: {made.stderr[-300:]!r}"
    view = Path(json.loads(cozy(args.cli, "--json", "vault", "status", cwd=encrypted).stdout)["view"])
    try:
        facts["encrypted"] = project_case(args.cli, args.out, encrypted, view)
    finally:
        cozy(args.cli, "--json", "vault", "lock", cwd=encrypted)

    (args.out / "kept-latents.json").write_text(json.dumps(facts, indent=2))
    print(f"kept latents: {json.dumps(facts)}", flush=True)
    for kind, case in facts.items():
        assert isinstance(case, dict)
        assert case["latents_in_files"], f"{kind}: a local run's latents aren't in its project's files/"
        assert not case["store_copies_after_upkeep"], f"{kind}: the machine store kept its copy past upkeep, so nothing here proves files/ serves it"
        assert case["read_latents"], f"{kind}: the take's latents weren't read after the store's copy went: {case.get('read_failed', '')}"
        assert case["gone_latents_landed"], f"{kind}: the take to delete kept no latents in files/, so its going proves nothing"
        assert not case["gone_loose_rows"], f"{kind}: the take's latents, landed after its run arrived, left {case['gone_loose_rows']} loose run row(s)"
        assert case["deleted"] and not case["latents_after_delete"], f"{kind}: the deleted take's latents stayed in files/: {case['latents_after_delete']}"


if __name__ == "__main__":
    main()
