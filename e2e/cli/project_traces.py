#!/usr/bin/env python3
"""project_traces (cl-1031): a project deleted (its folder removed) or locked leaves nothing of it on the computer outside its
folder. Paul (11-15): "when the project is deleted or locked, all run data goes with it."

Each project runs the e2e-scratch probe with a marker of its own as its prompt, BiRefNet on an image of its own (so its input's and
output's digests are its own too), and keeps a box of its own words. Then the ordinary project's folder goes away and Calcifer
lists its projects, which is how it finds a folder gone; a marker scan (containment/scan.py) of the cozy home, the
machine's stores and the TensorFS store finds nothing of it: its id, its name, its prompt, its box's words, its files' digests.
Only its folder's path may stay. A folder that went away isn't always deleted (a moved folder, a drive unplugged): put back and
opened, it is as it was, its key, its runs by KEY-N and its boxes, from its own project.db. Then it is deleted, and the scan finds
nothing again. The encrypted one is locked, and the same scan finds nothing of it outside its folder."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent / "containment"))
from private_execution import command  # noqa: E402
from project_http import Refused, request  # noqa: E402
from scan import Scanner, places  # noqa: E402

PROBE = "e2e-scratch/probe"
BIREFNET = "birefnet/remove_background"


def document(cli: Path, *args: str, cwd: Path | None = None) -> dict[str, object]:
    done = subprocess.run([str(cli), "--json", *args], cwd=cwd, capture_output=True, check=False)
    assert done.returncode == 0, f"cozy {' '.join(args)}: {done.stderr[-300:]!r}"
    return cast(dict[str, object], json.loads(done.stdout))  # cast: the CLI's --json document


def held(files: Path) -> set[str]:
    """The digests a project's files are kept under (`files/<sha256>.<ext>`)."""
    if not files.is_dir():
        return set()
    return {path.name.split(".")[0] for path in files.iterdir() if len(path.name.split(".")[0]) == 64}


def notice(project_id: str) -> bool:
    """Calcifer lists its projects, as the page and `cozy doctor` do: how it finds a folder gone. Whether it knows `project_id` after."""
    request("/v1/projects?limit=1000")
    try:
        request(f"/v1/projects/{project_id}/address")
    except Refused:
        return False
    return True


def project(cli: Path, out: Path, folder: Path, view: Path, image: Path, kind: str) -> dict[str, object]:
    """Its prompt and box markers and its files' digests, after a probe run, a BiRefNet run and a box of its own."""
    marker, words = f"CL1031_{kind}_{secrets.token_hex(12)}", f"CL1031_{kind}_BOX_{secrets.token_hex(12)}"
    # An image of its own: the picture with bytes of its own after it, which a decoder ignores.
    mine = out / f"{kind}.jpg"
    mine.write_bytes(image.read_bytes() + secrets.token_bytes(32))
    before = held(view / "files")
    command(cli, out, f"{kind}-probe", "run", PROBE, f"marker={marker}", "--on", "local", cwd=folder)
    command(cli, out, f"{kind}-birefnet", "run", BIREFNET, "--file", f"image={mine}", "--on", "local", cwd=folder)
    digests = {hashlib.sha256(mine.read_bytes()).hexdigest()} | (held(view / "files") - before)
    assert len(digests) >= 2, f"the {kind} project kept no output of BiRefNet's in its files: {sorted(held(view / 'files'))}"
    box = out / f"{kind}-box.json"
    box.write_text(json.dumps({f"note_{kind.lower()}": {"kind": "text", "text": words}}))
    command(cli, out, f"{kind}-apply", "apply", "-f", str(box), cwd=folder)
    return {"markers": [marker, words], "digests": sorted(digests)}


def state(cli: Path, folder: Path) -> dict[str, object]:
    """What a person sees of the project: its key, its runs by name and state, its boxes."""
    runs: list[list[str]] = []

    def named(value: object) -> None:
        if isinstance(value, dict):
            if isinstance(value.get("name"), str) and "status" in value:
                runs.append([value["name"], str(value["status"])])
            for item in value.values():
                named(item)
        elif isinstance(value, list):
            for item in value:
                named(item)

    named(document(cli, "run", "ls", "--limit", "0", cwd=folder))
    return {"key": document(cli, "key", cwd=folder).get("key"), "runs": sorted(runs), "boxes": document(cli, "get", "--all", cwd=folder)}


def scanner_for(folder: Path, identity: dict[str, object]) -> Scanner:
    scanner = Scanner()
    for marker in cast(list[str], identity["markers"]):
        scanner.add(marker)
    for digest in cast(list[str], identity["digests"]):
        scanner.add(digest); scanner.add("sha256:" + digest)
    scanner.identity(folder, str(identity["id"]), folder.name)
    return scanner


def traces(scanner: Scanner, roots: list[Path], excluded: list[Path], receipt: Path, since: int, label: str) -> dict[str, object]:
    """The scan, again for a while as what's left goes; where a match is, by path (the markers are this cell's own)."""
    located = places(Path.home(), Path(os.environ.get("RUNNER_TEMP", "/tmp")))
    result: dict[str, object] = {}
    for _ in range(4):
        result = scanner.scan(roots, excluded, receipt, located, since_ns=since)
        if not result["matches"]:
            return result
        time.sleep(10)
    hashed = set(cast(list[str], result["path_sha256"]))
    found = []
    for root in roots:
        for directory, dirs, names in os.walk(root, followlinks=False):
            for name in dirs + names:
                path = Path(directory) / name
                if hashlib.sha256(os.fsencode(path)).hexdigest() in hashed:
                    found.append(str(path))
    result["paths"] = sorted(found)
    print(f"project traces: {label}: {json.dumps({k: v for k, v in result.items() if k.startswith('match') or k == 'paths'})}", flush=True)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--passphrase", required=True, type=Path)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    since = time.time_ns() - 2_000_000_000
    home = Path.home()
    store = Path(os.environ.get("TENSORFS_HOME", str(home / ".cache/tensorfs")))
    roots = [home / ".cozy", store, home / ".cache", home / ".config", home / ".local", Path("/tmp")]
    excluded = [home / ".cozy/calcifer/vaults", args.out.resolve(), args.work.resolve(), args.passphrase.resolve().parent]
    facts: dict[str, object] = {}

    deleted = args.work / f"deleted-{secrets.token_hex(8)}"
    made = document(args.cli, "new", "--unencrypted", str(deleted))
    gone = project(args.cli, args.out, deleted, deleted, args.image, "DELETED") | {"id": made["id"], "name": deleted.name}

    locked = args.work / f"locked-{secrets.token_hex(8)}"
    made = document(args.cli, "new", str(locked), "--passphrase-file", str(args.passphrase), "--recovery-out", str(args.out / "recovery"))
    view = Path(str(document(args.cli, "vault", "status", cwd=locked)["view"]))
    closed = project(args.cli, args.out, locked, view, args.image, "LOCKED") | {"id": made["id"], "name": locked.name}

    # Away: its folder moved out of reach (as an unplugged drive takes it), Calcifer lists its projects, which finds it gone;
    # nothing of it stays. Put back and opened, it is as it was: everything of value was in the folder.
    was = state(args.cli, deleted)
    away = args.work / "away"
    away.mkdir()
    deleted.rename(away / deleted.name)
    facts["away_still_known"] = notice(str(gone["id"]))
    facts["away"] = traces(scanner_for(deleted, gone), roots, excluded, args.out / "away-receipt.json", since, "away")
    (away / deleted.name).rename(deleted)
    back = state(args.cli, deleted)
    facts["back"] = {"was": was, "back": back}

    # Deleted: its folder removed, and the scan finds nothing again.
    subprocess.run(["rm", "-rf", "--", str(deleted)], check=True)
    facts["deleted_still_known"] = notice(str(gone["id"]))
    facts["deleted"] = traces(scanner_for(deleted, gone), roots, excluded, args.out / "deleted-receipt.json", since, "deleted")

    # Locked: its runs ended, its view closed, nothing of it outside its folder.
    done = subprocess.run([str(args.cli), "--json", "vault", "lock"], cwd=locked, capture_output=True, check=False)
    assert done.returncode == 0, f"vault lock: {done.stderr[-300:]!r}"
    for _ in range(120):
        if document(args.cli, "vault", "status", cwd=locked).get("state") == "locked":
            break
        time.sleep(0.5)
    else:
        raise AssertionError("the encrypted project did not lock")
    facts["locked"] = traces(scanner_for(locked, closed), [*roots, locked], [path for path in excluded if path != args.work.resolve()] + [args.work.resolve() / deleted.name, away.resolve()],
                             args.out / "locked-receipt.json", since, "locked")

    (args.out / "project-traces.json").write_text(json.dumps(facts, indent=2))
    print(f"project traces: back: {json.dumps(facts['back'])[:2000]}", flush=True)
    left = {kind: result for kind, result in facts.items() if isinstance(result, dict) and kind != "back" and result["matches"]}
    assert not facts["away_still_known"] and not facts["deleted_still_known"], f"Calcifer's listing didn't let a gone project go: {facts['away_still_known']}, {facts['deleted_still_known']}"
    assert back == was, f"the project put back and opened isn't as it was: {json.dumps(was)[:600]} then {json.dumps(back)[:600]}"
    assert not left, "project traces: " + "; ".join(
        f"{kind}: {result['matches']} places, {', '.join(result.get('paths', [])[:6]) or 'paths unknown'}" for kind, result in left.items())
    print(f"project traces: none left of the project away, deleted or locked, and back as it was ({json.dumps({k: v.get('files') for k, v in facts.items() if isinstance(v, dict) and k != 'back'})})", flush=True)


if __name__ == "__main__":
    main()
