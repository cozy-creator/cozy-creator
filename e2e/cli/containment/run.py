# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=11", "python-snappy>=0.7"]
# ///
"""Hosted only: real installed CLI/Calcifer/OwnedBrowser, with private values retained only in RAM.

Coordinator invocation, only after the composed candidate is ready:
  uv run --no-project e2e/cli/containment/run.py --ready-file READY.json --out RECEIPT_DIR
This deliberately does not use e2e's argv/body/log capture wrappers.
"""
from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import sqlite3
import subprocess
import sys
import time
from urllib.parse import quote

from common import Failure, Native, Proof, Receipt, alive, birth, kill_exact, project_route, require, sha, until
from scan import Scanner, tmpfs
from native_execution import CorePattern, qualify as qualify_native

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from test_hub import require_test_hub


def new_png(scanner: Scanner, marker: str) -> bytes:
    from PIL import Image, PngImagePlugin
    bits = bytes(secrets.randbits(1) for _ in range(256))
    scanner.images.add(bits)
    image = Image.new("RGB", (256, 256))
    pixels = image.load()
    for y in range(256):
        for x in range(256):
            shade = 255 if bits[y // 16 * 16 + x // 16] else 0
            pixels[x, y] = (shade, shade, shade)
    metadata = PngImagePlugin.PngInfo()
    metadata.add_text("Description", marker)
    result = io.BytesIO()
    image.save(result, format="PNG", pnginfo=metadata)
    return result.getvalue()


class ForeignBrowsers:
    """Only new fixture-owned, data-free processes; never attach to an existing browser."""
    def __init__(self):
        self.held: list[tuple[subprocess.Popen, dict, Path]] = []
        program = next((shutil.which(name) for name in ("google-chrome", "chromium", "chromium-browser") if shutil.which(name)), None)
        require(program, "chrome_missing")
        parent = Path("/dev/shm")
        require(tmpfs(parent), "foreign_profile_tmpfs")
        for incognito in (False, True):
            profile = parent / ("containment-foreign-" + secrets.token_hex(12))
            profile.mkdir(mode=0o700)
            command = [program, "--user-data-dir=" + str(profile), "--no-first-run", "--no-default-browser-check", "--disable-crash-reporter", "--disable-breakpad", "--disable-background-networking"]
            if incognito:
                command.append("--incognito")
            command.append("about:blank")
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True, env={**os.environ, "TMPDIR": str(profile), "XDG_CACHE_HOME": str(profile / "cache")})
            exact = birth(process.pid)
            require(alive(exact), "foreign_browser_start")
            self.held.append((process, exact, profile))

    def preserved(self) -> bool:
        return len(self.held) == 2 and all(alive(exact) for _, exact, _ in self.held)

    def close(self) -> None:
        for process, exact, profile in self.held:
            if alive(exact):
                # The unreaped exact leader holds its process-group identity; these are exclusively
                # the fixture's two data-free processes, never a user browser or name match.
                require(birth(process.pid) == exact, "foreign_cleanup_birth")
                os.killpg(process.pid, 9)
            process.wait()
            shutil.rmtree(profile)


def scan_case(scanner: Scanner, receipt: Receipt, case: str, roots: list[Path], exclusions: list[Path]) -> None:
    result = scanner.scan(roots, exclusions, receipt.path)
    receipt.add(case, **result)
    require(result["matches"] == 0, "outside_project_canary")
    require(not any(result.get(key, 0) for key in ("unreadable", "sqlite_unreadable", "archive_unreadable", "archive_oversize", "leveldb_unreadable", "images_unreadable")), "scan_decoder_unavailable")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ready-file", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_OS") == "Linux" and sys.platform == "linux", "hosted_linux_only")
    require(os.environ.get("DISPLAY"), "hosted_display_missing")
    require_test_hub(os.environ.get("COZY_HUB_URL", ""), os.environ.get("COZY_TEST_HUB_ORIGIN", ""))
    cli = Path.home() / ".cozy/bin/cozy"
    ready = json.loads(args.ready_file.read_bytes())
    candidate = sha(cli.read_bytes())
    require(ready.get("ready") is True and ready.get("sha256") == candidate, "composed_candidate_not_ready")
    native, scanner = Native(cli), Scanner()
    receipt = Receipt(args.out.resolve() / "receipt.json", candidate)
    try:
        previous_holder = native.holder()
        require(not alive(birth(previous_holder["pid"])), "fixture_requires_stopped_calcifer")
    except (FileNotFoundError, ProcessLookupError):
        pass
    # A leftover lock record is not accepted as proof of ownership. The coordinator stops the
    # installed service before handing this fixture its exact ready candidate.
    root = Path(os.environ["RUNNER_TEMP"]) / ("containment-projects-" + secrets.token_hex(10))
    root.mkdir(mode=0o700)
    roots = [Path.home(), root, Path("/tmp"), Path("/var/tmp"), Path("/dev/shm"), Path(os.environ["RUNNER_TEMP"])]
    for name in ("TMPDIR", "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME"):
        if os.environ.get(name):
            roots.append(Path(os.environ[name]))
    proof: Proof | None = None
    foreign: ForeignBrowsers | None = None
    core: CorePattern | None = None
    projects: list[dict] = []
    phase = "start"
    try:
        if ready.get("native_execution") is True:
            core = CorePattern(ready)
            core.set(b"core")
        proof = Proof(cli)
        until(lambda: native.holder(), lambda holder: holder.get("pid") == proof.process.pid, "holder_ready")
        receipt.add("native_capability", accepted=native.call("GET", "/v1/project-storage").get("version") == 3)
        for slot in ("a", "b"):
            phase = "create_" + slot
            credential = "proof-" + secrets.token_hex(32)
            scanner.add(credential)
            folder = root / slot
            opened = native.document("new", str(folder), "--passphrase-file", "-", stdin=(credential + "\n").encode())
            require(isinstance(opened.get("id"), str), "encrypted_create_identity")
            if opened.get("recovery_secret"):
                scanner.add(opened["recovery_secret"])
            vault = native.document("vault", "status", cwd=folder)
            require(vault.get("encrypted") is True and vault.get("state") == "unlocked", "born_encrypted")
            view = Path(vault["view"]).resolve()
            vaults = Path.home() / ".cozy/calcifer/vaults"
            require(view.is_absolute() and view.is_dir() and view != vaults.resolve() and (view.is_relative_to(folder.resolve()) or view.is_relative_to(vaults.resolve())) and (view / "project.db").is_file(), "exact_unlocked_project_root")
            marker = "PRIVATE_" + secrets.token_hex(24)
            edited = "EDITED_" + secrets.token_hex(24)
            asset = "private_" + secrets.token_hex(12)
            picture_asset = "image_" + secrets.token_hex(12)
            for value in (marker, edited, asset, picture_asset):
                scanner.add(value)
            (view / "root-data.txt").write_text(marker)
            (view / "files").mkdir(exist_ok=True)
            (view / "files/root-data.txt").write_text(marker)
            image = new_png(scanner, marker)
            digest = "sha256:" + sha(image)
            # The content hash itself is derived project data and must not become global custody.
            scanner.add(digest); scanner.add(digest.removeprefix("sha256:"))
            base = project_route(opened["id"])
            native.call("POST", base + "/assets", {"name": asset, "exact": True, "fields": {"text": marker}, "at": {"x": 0, "y": 0, "w": 320}})
            native.call("PUT", "/v1/project-files/" + digest + "?project=" + quote(opened["id"], safe=""), image)
            imported = native.call("POST", base + "/imports", {"source": digest, "name": picture_asset, "exact": True, "at": {"x": 400, "y": 0, "w": 320}})
            projects.append({"folder": folder, "view": view, "id": opened["id"], "credential": credential, "marker": marker, "edit": edited, "asset": asset, "path": imported["path"], "digest": digest})
        a, b = projects
        if core is not None:
            phase = "native_execution"
            qualify_native(native, scanner, a, receipt, core)
        phase = "browser_open"
        foreign = ForeignBrowsers()
        for project in projects:
            native.document("open", cwd=project["folder"])
            until(lambda: proof.exercise(project["id"], "ready"), lambda value: value.get("page_ready") and value.get("cards", 0) >= 2, "owned_page_ready")
        snapshot = proof.ask("snapshot")
        require(snapshot["present"] and snapshot["pages"] == 2 and len(snapshot.get("processes", [])) >= 2 and snapshot["birth"] in snapshot["processes"], "shared_owned_browser")
        browser_birth, profile = snapshot["birth"], Path(snapshot["profile"])
        require(profile.name.startswith("cozy-private-") and tmpfs(profile) and profile.stat().st_mode & 0o777 == 0o700 and profile.stat().st_uid == os.geteuid(), "owned_profile_volatile")
        argv = Path(f"/proc/{browser_birth['pid']}/cmdline").read_bytes().split(b"\0")
        require(b"--incognito" in argv and b"--remote-debugging-pipe" in argv and ("--user-data-dir=" + str(profile)).encode() in argv and not any(word.startswith(b"--remote-debugging-port") for word in argv), "actual_private_browser_flags")
        environment = dict(item.split(b"=", 1) for item in Path(f"/proc/{browser_birth['pid']}/environ").read_bytes().split(b"\0") if b"=" in item)
        require(all(key.encode() in environment and Path(os.fsdecode(environment[key.encode()])).is_relative_to(profile) for key in ("HOME", "TMPDIR", "TEMP", "TMP", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR")), "actual_private_browser_redirects")
        require(not any(marker in b"\0".join(argv) for marker in scanner.markers), "private_browser_argv_canary")
        require(all(browser_birth != held[1] for held in foreign.held), "browser_ownership_distinct")
        receipt.add("shared_owned_browser", pages=2, volatile=True, unrelated=foreign.preserved(), birth_sha256=sha(json.dumps(browser_birth, sort_keys=True).encode()))
        for project in projects:
            phase = "headers"
            base = project_route(project["id"])
            paths = [base + "/files/root-data.txt", base + "/" + project["path"], base + "/thumbs/" + project["digest"].removeprefix("sha256:") + "?w=128"]
            for number, path in enumerate(paths):
                observed = proof.exercise(project["id"], "media", path=path)
                require(observed.get("authenticated") and observed.get("status") == 200 and observed.get("no_store"), "owned_no_store_200")
                if number == 1:
                    ranged = proof.exercise(project["id"], "media", path=path, range=True)
                    conditional = proof.exercise(project["id"], "media", path=path, conditional=True)
                    require(ranged.get("status") == 206 and ranged.get("no_store") and ranged.get("range"), "owned_no_store_206")
                    require(conditional.get("status") in {200, 304} and conditional.get("no_store"), "owned_no_store_conditional")
                    receipt.add("media_" + ("a" if project is a else "b"), status=200, range_status=ranged["status"], conditional_status=conditional["status"], body_sha256=observed["body_sha256"])
            storage = proof.exercise(project["id"], "storage")
            require(storage == {"project_preferences": 0, "durable_session": False, "indexed_databases": 0, "cache_stores": 0, "cookies": 0}, "browser_durable_storage")
        refused = native.raw("GET", project_route(a["id"], "/files/" + a["marker"]), extra={"Origin": "http://" + a["marker"] + ".invalid"})
        require(refused.status == 403 and "no-store" in refused.headers.get("cache-control", ""), "csrf_no_store_refusal")
        unauthenticated = native.raw("GET", project_route(a["id"]), auth=False)
        require(unauthenticated.status == 401, "native_auth_required")
        scan_case(scanner, receipt, "live_outside_scan", roots, [a["view"], b["view"], profile])
        phase = "queued_b_edit"
        held = sqlite3.connect(b["view"] / "project.db", timeout=1)
        held.execute("BEGIN IMMEDIATE")
        locking = None
        try:
            until(lambda: proof.exercise(b["id"], "edit", asset=b["asset"], field="text", value=b["edit"]), lambda value: value.get("typed"), "real_browser_edit")
            require(proof.exercise(b["id"], "observe", needle=b["edit"]).get("visible"), "browser_edit_not_visible")
            locking = subprocess.Popen([str(cli), "--json", "vault", "lock"], cwd=a["folder"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            time.sleep(0.5)
            if locking.poll() is not None:
                early, _ = locking.communicate()
                require(locking.returncode != 0 and json.loads(early).get("error", {}).get("code") == "project_busy", "lock_discarded_pending_b_edit")
            before = native.document("vault", "status", cwd=a["folder"])
            require(before.get("state") == "unlocked", "lock_closed_admission_before_browser_drain")
        finally:
            held.rollback(); held.close()
        require(locking is not None, "lock_not_started")
        stdout, stderr = locking.communicate(timeout=90)
        busy = locking.returncode != 0
        if busy:
            require(json.loads(stdout).get("error", {}).get("code") == "project_busy", "a_lock_unexpected_refusal")
            require(proof.exercise(b["id"], "observe", needle=b["edit"]).get("visible"), "busy_lock_lost_b_draft")
            # An orderly busy refusal is valid. Finish the same real UI edit, then retry A's lock.
            until(lambda: proof.exercise(b["id"], "edit", asset=b["asset"], field="text", value=b["edit"]), lambda value: value.get("typed"), "busy_edit_resume")
            until(lambda: native.call("GET", project_route(b["id"], "/assets/" + quote(b["asset"], safe="")))["asset"]["fields"]["text"], lambda value: value == b["edit"], "b_edit_not_persisted")
            stdout = native.command("vault", "lock", cwd=a["folder"])
        require(json.loads(stdout)["vault"]["state"] == "locked", "a_lock_after_b_saved")
        saved = native.call("GET", project_route(b["id"], "/assets/" + quote(b["asset"], safe="")))
        require(saved["asset"]["fields"]["text"] == b["edit"], "b_edit_not_persisted")
        until(lambda: [alive(process) for process in snapshot["processes"]], lambda live: not any(live), "owned_browser_tree_not_retired")
        require(not profile.exists() and foreign.preserved() and not (a["view"] / "root-data.txt").exists(), "whole_browser_retirement")
        receipt.add("queued_b_edit_then_a_lock", saved=True, owned_retired=True, profile_removed=True, unrelated_preserved=True, busy_refusal=busy)
        scan_case(scanner, receipt, "a_locked_outside_scan", roots, [b["view"]])
        phase = "last_window_close"
        native.document("open", cwd=b["folder"])
        until(lambda: proof.exercise(b["id"], "ready"), lambda value: value.get("page_ready"), "reopened_page_ready")
        last = proof.ask("snapshot")
        try:
            proof.ask("close_owned")
        except Failure:
            pass  # Closing the final page can close its pipe before the response; observe the birth.
        until(lambda: [alive(process) for process in last["processes"]], lambda live: not any(live), "last_window_process_tree_alive")
        until(lambda: Path(last["profile"]).exists(), lambda exists: not exists, "last_window_profile_left")
        require(foreign.preserved(), "foreign_browser_changed")
        receipt.add("last_window_close", process_gone=True, profile_removed=True, unrelated_preserved=True)
        phase = "browser_crash"
        native.document("open", cwd=b["folder"])
        until(lambda: proof.exercise(b["id"], "ready"), lambda value: value.get("page_ready"), "crash_page_ready")
        crashed = proof.ask("snapshot")
        kill_exact(crashed["birth"])
        until(lambda: [alive(process) for process in crashed["processes"]], lambda live: not any(live), "crashed_browser_tree_not_retired")
        until(lambda: Path(crashed["profile"]).exists(), lambda exists: not exists, "crashed_profile_not_retired")
        require(foreign.preserved(), "foreign_browser_changed")
        receipt.add("browser_crash", process_gone=not alive(crashed["birth"]), profile_removed=True, unrelated_preserved=True)
        phase = "cold_restart"
        native.document("vault", "lock", cwd=b["folder"])
        previous = native.holder()
        native.call("POST", "/v1/down", {})
        proof.process.wait(timeout=90); proof.close_channel(); proof = None
        proof = Proof(cli)
        until(lambda: native.holder(), lambda value: value.get("pid") == proof.process.pid, "restart_holder_ready")
        rejected = native.raw("GET", "/v1/project-storage", extra={"Authorization": "Bearer " + previous["token"]}, preflight=False)
        require(rejected.status == 401, "old_holder_authority_survived")
        native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=b["folder"], stdin=(b["credential"] + "\n").encode())
        native.document("open", cwd=b["folder"])
        until(lambda: proof.exercise(b["id"], "ready"), lambda value: value.get("page_ready"), "cold_page_ready")
        fresh = proof.ask("snapshot")
        require(fresh["birth"] not in (browser_birth, last["birth"], crashed["birth"]), "browser_birth_reused")
        if a.get("native_run_id"):
            native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=a["folder"], stdin=(a["credential"] + "\n").encode())
            reopened = native.document("vault", "status", cwd=a["folder"])
            require(sha((Path(reopened["view"]) / "native-allowed.txt").read_bytes()) == a["native_allowed_sha256"], "native_file_not_retained_after_restart")
            retained = native.call("GET", project_route(a["id"], "/runs/" + quote(a["native_run_id"], safe="")))
            require(retained.get("status") == "completed" and retained.get("result", {}).get("allowed_sha256") == a["native_allowed_sha256"], "native_history_not_retained_after_restart")
            native.document("vault", "lock", cwd=a["folder"])
            receipt.add("native_lock_restart_retention", file_retained=True, history_retained=True, output_sha256=a["native_allowed_sha256"])
        native.document("vault", "lock", cwd=b["folder"])
        receipt.add("cold_restart", new_authority=True, new_browser=True, unrelated_preserved=foreign.preserved())
        scan_case(scanner, receipt, "all_locked_outside_scan", roots, [])
        health = (Path.home() / ".cozy/calcifer/health.log").read_bytes()
        require(b"started" in health and all(marker not in health for marker in scanner.markers), "closed_health_sink")
        receipt.add("health_sink", events=health.count(b"INFO"), canaries=0)
        receipt.value["complete"] = True
        receipt.save()
        return 0
    except BaseException as error:
        # Exception messages, response reprs, SQL values and command output can contain canaries.
        receipt.value["failed_case"] = phase
        receipt.value["error_class"] = type(error).__name__
        receipt.save()
        return 1
    finally:
        if proof is not None:
            try:
                for project in projects:
                    native.command("vault", "lock", cwd=project["folder"])
                native.call("POST", "/v1/down", {})
                proof.process.wait(timeout=30)
            except BaseException:
                receipt.value["cleanup_incomplete"] = True
                receipt.save()
            proof.close_channel()
        try:
            if foreign is not None:
                foreign.close()
        finally:
            if core is not None:
                try:
                    core.restore()
                    receipt.add("host_core_restoration", restored=True)
                except BaseException:
                    receipt.value["host_core_restore_failed"] = True
                    receipt.value["complete"] = False
                    receipt.save()
                    raise


if __name__ == "__main__":
    try:
        result = main()
    except BaseException:
        # Guard failures also stay sanitized; no traceback includes private request objects.
        print("hosted containment fixture unavailable", file=sys.stderr)
        result = 2
    raise SystemExit(result)
