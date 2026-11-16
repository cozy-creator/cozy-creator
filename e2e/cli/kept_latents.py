#!/usr/bin/env python3
"""kept_latents (cl-1046): a local run's latents are an ordinary file of its take, in its project. Coordinator (11-16): "the latent is
an ordinary file of its take inside the project, on the one export path, and purge already handles its lifetime"; Paul: a project's
files live in its folder.

In a plain project and an encrypted one: a box of project/kept.py's `shot` renders on local, once with small latents and once with
large ones (how much longer the large one takes is said); its latents are in its project's files/. Then the project's machine store is
aged past its TTL and grace, the local machine restarts and a render writes to that store (its upkeep collects what nothing holds): a
run naming the take's latents by their hash still reads them whole, from files/. Deleted, the take's latents file leaves files/ when its undo window
ends (Calcifer runs with a short one here)."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

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
    return json.loads(request(project_path(project, suffix), method, body))


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


def project_case(cli: Path, out: Path, project: Path, view: Path) -> dict[str, object]:
    facts: dict[str, object] = {}
    # A box per render, each its own request, so each runs: the first installs its environment, the next two are measured.
    for box, size in (("warm", 64), ("small", 65), ("large", LARGE), ("after", 66)):
        api(project, "/assets", "POST", {"name": box, "tool": f"{KEPT}/shot", "kind": "video", "exact": True, "fields": {"size": size}})
    render(cli, project, out, "warm")
    facts["small_s"] = round(render(cli, project, out, "small"), 2)
    facts["large_s"] = round(render(cli, project, out, "large"), 2)
    latents = digest(take_files(project, "large")["latents"])
    facts["latents_in_files"] = [str(path) for path in held(view / "files", latents)]
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

    # Deleted: its latents file leaves files/ once its undo window ends.
    deleted = cozy(cli, "--json", "delete", "large", cwd=project, out=out, name=f"delete-{project.name}")
    facts["deleted"] = deleted.returncode == 0
    for _ in range(90):
        if not held(view / "files", latents):
            break
        time.sleep(2)
    facts["latents_after_delete"] = [str(path) for path in held(view / "files", latents)]
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
        assert case["deleted"] and not case["latents_after_delete"], f"{kind}: the deleted take's latents stayed in files/: {case['latents_after_delete']}"


if __name__ == "__main__":
    main()
