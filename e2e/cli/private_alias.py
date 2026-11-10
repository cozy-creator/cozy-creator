#!/usr/bin/env python3
"""Real Linux private execution after its mounted view's canonical path changes.

Run after private_execution.py in a synthetic unlocked vault. The normal lock/unlock
commands own the same daemon and home. A runtime-directory alias changes only the
physical mounted view; project identity, source bytes and logical input references stay.
The fixture leaves that owned alias in place for subsequent rekey/lock qualification.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import time
import uuid

from private_execution import command, scan


def installations(home: Path, project_id: str) -> set[tuple[str, str, str, str]]:
    prefix = f"local-private:{len(project_id.encode())}:{project_id}:"
    rows: set[tuple[str, str, str, str]] = set()
    for path in home.rglob("executions.sqlite3"):
        if not path.is_file() or path.is_symlink():
            continue
        with sqlite3.connect(f"file:{path}?mode=ro", uri=True) as db:
            for actor, alias, record in db.execute("SELECT actor,alias,record FROM installations"):
                if actor.startswith(prefix):
                    rows.add((str(path), actor, alias, record))
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--view", required=True, type=Path)
    parser.add_argument("--passphrase-file", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    if os.name != "posix" or not Path("/proc").is_dir():
        raise SystemExit("controlled private alias execution qualification requires Linux")
    args.out.mkdir(parents=True, exist_ok=True)
    home = (Path.home() / ".cozy").resolve()
    view = args.view.resolve(strict=True)
    runtime = view.parent
    assert runtime.parent == (home / "calcifer/vaults").resolve(), "fixture must own one vault runtime directory"
    marker = json.loads((view / ".vault-local-execution.json").read_text())
    project_id = marker["storage"]["project_id"]
    folder = view / "private-execution-proof"
    script, request = folder / "probe.py", folder / "input.json"
    source_bytes = script.read_bytes()
    source_canary = re.search(rb"XS160_SOURCE_[0-9a-f]{32}", source_bytes)
    assert source_canary, "first run the owned synthetic private_execution.py fixture"
    before = installations(home, project_id)
    assert before, "private local-code installation was not retained before remount"
    stamp = time.time_ns() - 2_000_000_000

    locked = json.loads(command(args.cli, args.out, "lock", "project", "-p", str(args.project), "lock"))
    assert locked["vault"]["state"] == "locked", "fixture cannot relocate an active mounted view"
    relocated = runtime.with_name(f"alias-proof-{uuid.uuid4().hex}")
    runtime.rename(relocated)
    runtime.symlink_to(relocated, target_is_directory=True)
    unlocked = json.loads(command(args.cli, args.out, "unlock", "project", "-p", str(args.project), "unlock",
                                  "--passphrase-file", str(args.passphrase_file), "--idle", "0"))
    after_view = Path(unlocked["vault"]["view"]).resolve(strict=True)
    assert after_view != view, "the native view's canonical path did not change"
    assert json.loads((after_view / ".vault-local-execution.json").read_text())["storage"]["project_id"] == project_id
    assert hashlib.sha256(script.read_bytes()).digest() == hashlib.sha256(source_bytes).digest(), "source bytes changed"
    payload = json.loads(request.read_text())
    payload_canary = f"XS160_ALIAS_PAYLOAD_{uuid.uuid4().hex}"
    payload.update(text=payload_canary, fail=False)
    request.write_text(json.dumps(payload))
    result = command(args.cli, args.out, "remounted-run", "run", "-p", str(args.project), str(script),
                     "--input", str(request), "--on", "local")
    assert payload_canary.encode() in result, "remounted private call did not return its fresh input"
    assert b'"scratch_private":true' in result.replace(b" ", b""), "remounted execution escaped protected storage"
    after = installations(home, project_id)
    assert before.issubset(after), "remount rewrote/deleted historical installation bindings"
    assert len(after) > len(before), "the new physical mount did not receive its own installation binding"
    markers = [source_canary.group(), payload_canary.encode()]
    store = Path(os.environ.get("TENSORFS_HOME", str(Path.home() / ".cache/tensorfs")))
    leaked, checked = scan([home, store], [home / "calcifer/vaults", args.out.resolve()], stamp, markers)
    (args.out / "scan.json").write_text(json.dumps({"checked_new_or_modified_files": checked, "leaked_paths": leaked}, indent=2))
    assert not leaked, "remount exposed private content in global storage"
    (args.out / "receipt.json").write_text(json.dumps({
        "same_project": True, "same_source": True, "changed_physical_view": True,
        "new_binding": True, "historical_bindings_retained": True,
        "actual_private_execution": True, "global_plaintext_scan": True,
        "synthetic_only": True,
    }, indent=2))


if __name__ == "__main__":
    main()
