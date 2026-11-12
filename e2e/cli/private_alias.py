#!/usr/bin/env python3
"""Real Linux private execution after its mounted view's canonical path changes.

Run after private_execution.py in a synthetic unlocked vault. The normal lock/unlock
commands own the same daemon and home. A normal fresh unlock changes only the
physical mounted view; project identity, source bytes and input file bytes stay.
The caller continues with the newly unlocked view for rekey/lock qualification.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import time
import uuid

from private_execution import command, observed_run, scan, target_history


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
    parser.add_argument("--passphrase-file", type=Path)
    verify = parser.add_mutually_exclusive_group()
    verify.add_argument("--verify-rekey", action="store_true", help="verify retained target history on the caller's already-rotated mount; execute no work")
    verify.add_argument("--verify-decrypt", action="store_true", help="verify and run a retained target after the project becomes plaintext")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    if os.name != "posix" or not Path("/proc").is_dir():
        raise SystemExit("controlled private alias execution qualification requires Linux")
    args.out.mkdir(parents=True, exist_ok=True)
    home = (Path.home() / ".cozy").resolve()
    view = args.view.resolve(strict=True)
    marker = json.loads((view / ".vault-local-execution.json").read_text())
    project_id = marker["storage"]["project_id"]
    folder = view / "private-execution-proof"
    script, request = folder / "probe.py", folder / "input.json"
    proof = json.loads((folder / "target-proof.json").read_text())
    assert proof["project_id"] == project_id, "private target proof belongs to another project"
    if args.verify_decrypt:
        status = json.loads(command(args.cli, args.out, "plain-status", "vault", "status", cwd=args.project))
        assert status["encrypted"] is False and status["state"] == "plain", "the retained target needs a decrypted project"
        assert view == args.project.resolve(), "the decrypted files are not in the original project directory"
        assert Path(marker["storage"]["view_root"]).resolve() == view, "decryption retained an obsolete mounted path"
        ids = target_history(args.cli, args.out, "decrypted-target-history", proof)
        assert ids == proof["ids"], "decryption changed retained run history"
        assert proof["function"].encode() in script.read_bytes(), "decryption changed the retained source"
        described = json.loads(command(args.cli, args.out, "decrypted-describe", "run", str(script), "--describe", "--on", "local", cwd=args.project))
        assert described.get("accelerator") is False, "the original retained source no longer resolves"
        payload = json.loads(request.read_text())
        payload_canary = f"XS160_DECRYPTED_PAYLOAD_{uuid.uuid4().hex}"
        payload.update(text=payload_canary, source=str(folder / "input.txt"), fail=False)
        request.write_text(json.dumps(payload))
        result = command(args.cli, args.out, "decrypted-run", "run", str(script), "--input", str(request), "--on", "local", cwd=args.project)
        assert payload_canary.encode() in result, "the retained target did not execute after decryption"
        rerun = observed_run(result)
        assert rerun["target"] == proof["target"], "decryption changed the retained target identity"
        proof["ids"].append(rerun["id"])
        proof["ids"] = target_history(args.cli, args.out, "decrypted-rerun-history", proof)
        (folder / "target-proof.json").write_text(json.dumps(proof))
        (args.out / "receipt.json").write_text(json.dumps({"plain_project": True, "same_project": True, "target_history_retained": True, "retained_source_resolves": True, "retained_target_executes": True, "synthetic_only": True}, indent=2))
        return
    if args.verify_rekey:
        assert marker["storage"]["generation"] > proof["generation"], "the target proof needs an actually rotated project generation"
        assert proof["function"].encode() in script.read_bytes(), "rekey changed the private source's target"
        ids = target_history(args.cli, args.out, "rekey-target-history", proof)
        assert ids == proof["ids"], "rekey changed private target history or generated work again"
        # The new keybag predates its verified copy and global journal rewrap.
        stamp = max(0, (args.project / "project.vault.keys.json").stat().st_mtime_ns - 2_000_000_000)
        store = Path(os.environ.get("TENSORFS_HOME", str(Path.home() / ".cache/tensorfs")))
        leaked, checked = scan([home, store, args.project], [home / "calcifer/vaults", args.out.resolve()], stamp, [proof["function"].encode()])
        (args.out / "scan.json").write_text(json.dumps({"checked_new_or_modified_files": checked, "leaked_paths": leaked}, indent=2))
        assert not leaked, "rekey exposed private target metadata in global storage"
        (args.out / "receipt.json").write_text(json.dumps({"new_generation": True, "target_history_retained": True, "target_filter_pagination": True, "no_reexecution": True, "global_target_scan": True, "synthetic_only": True}, indent=2))
        return
    if args.passphrase_file is None:
        parser.error("--passphrase-file is required for the actual lock/remount proof")
    source_bytes = script.read_bytes()
    input_bytes = (folder / "input.txt").read_bytes()
    source_canary = re.search(rb"XS160_SOURCE_[0-9a-f]{32}", source_bytes)
    assert source_canary, "first run the owned synthetic private_execution.py fixture"
    before = installations(home, project_id)
    assert before, "private local-code installation was not retained before remount"
    def snapshot(rows: set[tuple[str, str, str, str]]) -> list[dict[str, object]]:
        return [{"journal": path, "actor": actor, "alias": alias, "record": json.loads(record)}
                for path, actor, alias, record in sorted(rows)]
    # These are protected references and public cache identities, not decrypted source,
    # credentials or request bodies. Retain exact evidence if the invariant ever fails.
    (args.out / "bindings-before.json").write_text(json.dumps(snapshot(before), indent=2))
    stamp = time.time_ns() - 2_000_000_000

    # A project's runs live in its own journal: Calcifer's global store never holds them.
    state = home / "calcifer/state.db"
    def global_runs() -> int:
        with closing(sqlite3.connect(f"file:{state}?mode=ro", uri=True)) as db:
            return db.execute(f"SELECT COUNT(*) FROM runs WHERE id IN ({','.join('?' * len(proof['ids']))})", proof["ids"]).fetchone()[0]
    assert global_runs() == 0, "private project runs reached Calcifer's global store"

    locked = json.loads(command(args.cli, args.out, "lock", "vault", "lock", cwd=args.project))
    assert locked["vault"]["state"] == "locked", "fixture must close its first mounted view"
    target_history(args.cli, args.out, "locked-target-history", proof, locked=True)
    unlocked = json.loads(command(args.cli, args.out, "unlock", "vault", "unlock",
                                  "--passphrase-file", str(args.passphrase_file), "--idle", "0", cwd=args.project))
    after_view = Path(unlocked["vault"]["view"]).resolve(strict=True)
    assert after_view != view, "the native view's canonical path did not change"
    assert json.loads((after_view / ".vault-local-execution.json").read_text())["storage"]["project_id"] == project_id
    folder = after_view / "private-execution-proof"
    script, request = folder / "probe.py", folder / "input.json"
    assert global_runs() == 0, "unlock copied private project runs into Calcifer's global store"
    assert hashlib.sha256(script.read_bytes()).digest() == hashlib.sha256(source_bytes).digest(), "source bytes changed"
    assert (folder / "input.txt").read_bytes() == input_bytes, "input file bytes changed"
    payload = json.loads(request.read_text())
    payload_canary = f"XS160_ALIAS_PAYLOAD_{uuid.uuid4().hex}"
    payload.update(text=payload_canary, source=str(folder / "input.txt"), fail=False)
    request.write_text(json.dumps(payload))
    result = command(args.cli, args.out, "remounted-run", "run", str(script),
                     "--input", str(request), "--on", "local", cwd=args.project)
    assert payload_canary.encode() in result, "remounted private call did not return its fresh input"
    assert b'"scratch_private":true' in result.replace(b" ", b""), "remounted execution escaped protected storage"
    remounted = observed_run(result)
    assert remounted["target"] == proof["target"], "remount changed the authorized private target"
    proof["ids"].append(remounted["id"])
    proof["ids"] = target_history(args.cli, args.out, "remount-target-history", proof)
    (folder / "target-proof.json").write_text(json.dumps(proof))
    after = installations(home, project_id)
    (args.out / "bindings-after.json").write_text(json.dumps(snapshot(after), indent=2))
    assert before.issubset(after), "remount rewrote/deleted historical installation bindings"
    assert len(after) > len(before), "the new physical mount did not receive its own installation binding"
    markers = [source_canary.group(), payload_canary.encode(), proof["function"].encode()]
    store = Path(os.environ.get("TENSORFS_HOME", str(Path.home() / ".cache/tensorfs")))
    leaked, checked = scan([home, store], [home / "calcifer/vaults", args.out.resolve()], stamp, markers)
    (args.out / "scan.json").write_text(json.dumps({"checked_new_or_modified_files": checked, "leaked_paths": leaked}, indent=2))
    assert not leaked, "remount exposed private content in global storage"
    (args.out / "receipt.json").write_text(json.dumps({
        "same_project": True, "same_source": True, "changed_physical_view": True,
        "new_binding": True, "historical_bindings_retained": True,
        "actual_private_execution": True, "global_plaintext_scan": True,
        "target_hidden_when_locked": True, "target_filter_pagination": True,
        "global_store_free_of_project_runs": True,
        "synthetic_only": True,
    }, indent=2))


if __name__ == "__main__":
    main()
