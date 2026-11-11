#!/usr/bin/env python3
"""Real Linux private execution after its mounted view's canonical path changes.

Run after private_execution.py in a synthetic unlocked vault. The normal lock/unlock
commands own the same daemon and home. A normal fresh unlock changes only the
physical mounted view; project identity, source bytes and input file bytes stay.
The caller continues with the newly unlocked view for rekey/lock qualification.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
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
    def snapshot(rows):
        return [{"journal": path, "actor": actor, "alias": alias, "record": json.loads(record)}
                for path, actor, alias, record in sorted(rows)]
    # These are protected references and public cache identities, not decrypted source,
    # credentials or request bodies. Retain exact evidence if the invariant ever fails.
    (args.out / "bindings-before.json").write_text(json.dumps(snapshot(before), indent=2))
    stamp = time.time_ns() - 2_000_000_000

    # Reproduce the exact older representation: protected spec, plaintext target.
    # Touch only a completed run this fresh, synthetic CLI fixture just created.
    runner_temp = os.environ.get("RUNNER_TEMP")
    assert os.environ.get("GITHUB_ACTIONS") == "true" and runner_temp and args.out.resolve().is_relative_to(Path(runner_temp).resolve()), "legacy target mutation requires the owned fresh CI fixture"
    state = home / "calcifer/state.db"
    prefix = b"cozy:vault:journal:2:"
    run_id = proof["ids"][0]
    with sqlite3.connect(f"file:{state}?mode=rw", uri=True) as db:
        db.execute("BEGIN IMMEDIATE")
        target, spec, out_dir, status = db.execute("SELECT target,spec,out_dir,state FROM runs WHERE id = ?", (run_id,)).fetchone()
        assert status == "settled" and Path(out_dir) == args.project.resolve() / ".runs", "legacy target fixture selected a foreign or active run"
        assert isinstance(spec, bytes) and spec.startswith(prefix) and target.encode().startswith(prefix), "legacy target fixture needs an already-protected run"
        changed = db.execute("UPDATE runs SET target = ? WHERE id = ? AND target = ?", (proof["target"], run_id, target)).rowcount
        assert changed == 1, "legacy target fixture did not change exactly its own slot"
    legacy_receipt = {"restored": True}
    (folder / "legacy-target-encoding.json").write_text(json.dumps(legacy_receipt))
    (args.out / "legacy-target-encoding.json").write_text(json.dumps(legacy_receipt))

    locked = json.loads(command(args.cli, args.out, "lock", "vault", "lock", cwd=args.project))
    assert locked["vault"]["state"] == "locked", "fixture must close its first mounted view"
    target_history(args.cli, args.out, "locked-target-history", proof, locked=True)
    reader = sqlite3.connect(f"file:{state}?mode=ro", uri=True)
    try:
        reader.execute("BEGIN")
        assert reader.execute("SELECT target FROM runs WHERE id = ?", (run_id,)).fetchone()[0] == proof["target"], "cleanup retry fixture did not pin the legacy target"
        print("vault proof: unlock with a held legacy SQL snapshot started", flush=True)
        blocked = subprocess.run([str(args.cli), "--json", "vault", "unlock", "--passphrase-file", str(args.passphrase_file), "--idle", "0"], capture_output=True, cwd=args.project)
        print(f"vault proof: unlock with a held legacy SQL snapshot exited {blocked.returncode}", flush=True)
        assert blocked.returncode != 0 and json.loads(blocked.stdout).get("error", {}).get("code") == "vault_journal_cleanup", "a pinned legacy snapshot did not refuse journal cleanup with its typed error"
        # The first unlock committed the new slot before its checkpoint failed. Its
        # ordinary retry must therefore finish cleanup even when no slot changes again.
        with sqlite3.connect(f"file:{state}?mode=ro", uri=True) as db:
            target = db.execute("SELECT target FROM runs WHERE id = ?", (run_id,)).fetchone()[0]
            assert target.encode().startswith(prefix), "the blocked cleanup did not commit its protected target first"
        legacy_receipt["cleanup_blocked_by_reader"] = True
    finally:
        reader.rollback()
        reader.close()
    unlocked = json.loads(command(args.cli, args.out, "unlock", "vault", "unlock",
                                  "--passphrase-file", str(args.passphrase_file), "--idle", "0", cwd=args.project))
    after_view = Path(unlocked["vault"]["view"]).resolve(strict=True)
    assert after_view != view, "the native view's canonical path did not change"
    assert json.loads((after_view / ".vault-local-execution.json").read_text())["storage"]["project_id"] == project_id
    folder = after_view / "private-execution-proof"
    script, request = folder / "probe.py", folder / "input.json"
    with sqlite3.connect(f"file:{state}?mode=ro", uri=True) as db:
        target = db.execute("SELECT target FROM runs WHERE id = ?", (run_id,)).fetchone()[0]
        assert target.encode().startswith(prefix), "normal unlock did not protect the legacy target slot"
    legacy_receipt["resealed"] = True
    legacy_receipt["cleanup_retry"] = True
    (folder / "legacy-target-encoding.json").write_text(json.dumps(legacy_receipt))
    (args.out / "legacy-target-encoding.json").write_text(json.dumps(legacy_receipt))
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
        "legacy_target_migrated": True,
        "legacy_cleanup_retried": True,
        "synthetic_only": True,
    }, indent=2))


if __name__ == "__main__":
    main()
