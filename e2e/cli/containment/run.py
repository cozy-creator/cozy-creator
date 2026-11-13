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
from functools import partial
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import sqlite3
import stat
import subprocess
import sys
import time
from typing import NotRequired, TypedDict, cast
from urllib.parse import quote

from common import (HMR, Birth, Failure, Native, Observed, OtherAccount, Project, Proof, Receipt, RunDetail, VaultStatus, alive, birth,
                    daemon_birth, kill_exact, principal, project_route, require, sha, until, upgrade_headers)
from scan import Scanner, places, tmpfs
from native_execution import qualify as qualify_native
from development import Development
from remote_cpu import qualify as qualify_remote

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from test_hub import require_test_hub


class Created(TypedDict):
    id: str
    recovery_secret: NotRequired[str]



class Imported(TypedDict):
    path: str


class Asset(TypedDict):
    fields: dict[str, object]


class AssetAnswer(TypedDict):
    asset: Asset


def new_png(scanner: Scanner, marker: str) -> bytes:
    from PIL import Image, PngImagePlugin
    bits = bytes(secrets.randbits(1) for _ in range(256))
    scanner.images.add(bits)
    image = Image.new("RGB", (256, 256))
    pixels = image.load()
    if pixels is None:  # Image.new allocates its pixels
        raise Failure("fixture_failure")
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
    def __init__(self) -> None:
        self.held: list[tuple[subprocess.Popen[bytes], Birth, Path]] = []
        program = next((shutil.which(name) for name in ("google-chrome", "chromium", "chromium-browser") if shutil.which(name)), None)
        if not program:
            raise Failure("chrome_missing")
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


EDIT_FACTS = frozenset({
    "located", "focused", "editor_present", "editor_rendered", "here_cards", "focus_viewport", "focus_entering",
    "visible", "has_focus", "animation_frame", "inert", "hit_target", "hit_node",
    "mousedown", "click", "dblclick", "mousedown_in_node", "click_in_node", "dblclick_in_node",
    "centre_in_view", "centre_hit", "centre_on_pane", "centre_on_other_node", "centre_on_tools", "centre_on_top",
    "centre_on_zoom", "centre_on_drawer", "box_visible_point", "node_hidden",
})
PAGE_FACTS = frozenset({
    "blank", "error_page", "loopback", "reloaded", "navigation_status", "owned_injected", "vite_client", "refresh_preamble",
    "app_mounted", "gate_connecting", "gate_signed_out", "gate_unreachable", "gate_opening_project", "gate_project_failed",
    "gate_broke", "vite_overlay", "visible", "resources", "resources_failed", "resources_unanswered", "resources_403",
    "resources_404", "resources_503", "resources_504",
})


REDIRECTS = (b"HOME", b"TMPDIR", b"TEMP", b"TMP", b"XDG_CACHE_HOME", b"XDG_CONFIG_HOME", b"XDG_STATE_HOME", b"XDG_DATA_HOME", b"XDG_RUNTIME_DIR")


def launch_environments(processes: list[Birth]) -> list[dict[bytes, bytes]]:
    # Chrome's setproctitle zeroes its own environ block; its launcher's stdio relays in the
    # owned tree keep the exact launch environment. PATH marks an intact block, never a title tail.
    found: list[dict[bytes, bytes]] = []
    for exact in processes:
        try:
            raw = Path(f"/proc/{exact['pid']}/environ").read_bytes() if alive(exact) else b""
        except (FileNotFoundError, ProcessLookupError):
            continue
        if not alive(exact):
            continue
        environment = dict(item.split(b"=", 1) for item in raw.split(b"\0") if re.fullmatch(rb"[A-Za-z_][A-Za-z0-9_]*=.*", item, re.S))
        if b"PATH" in environment:
            found.append(environment)
    return found


def tmpdir_singleton(profile: Path) -> bool:
    # Chrome binds its singleton socket in a unique directory under TMPDIR and links it from the profile.
    try:
        target = Path(os.readlink(profile / "SingletonSocket"))
        return target.is_absolute() and target.name == "SingletonSocket" and target.parent.parent == profile / "tmp" and stat.S_ISSOCK(target.lstat().st_mode)
    except OSError:
        return False


BUSY_MESSAGES = {
    "busy_browser_unsaved": "The Cozy browser has not confirmed", "busy_request": "a project request or tool is still finishing",
    "busy_work_stopping": "the project's work is still stopping", "busy_rendering": "The project is rendering, cutting or transferring",
    "busy_database_reader": "a database reader is preventing", "busy_unmount": "close programs using the project",
    "busy_transition_held": "Close applications using the project",
    "busy_storage_changing": "the project's storage is changing",
}


def busy_facts(document: dict[str, object]) -> dict[str, bool]:
    # Fixed product sentences become closed booleans; the message itself never enters the receipt.
    error = document.get("error")
    message = error.get("message", "") if isinstance(error, dict) else ""
    message = message if isinstance(message, str) else ""
    return {key: text in message for key, text in BUSY_MESSAGES.items()}


def view_holders(view: Path, calcifer: int, chrome: set[int]) -> dict[str, int]:
    # Counts only: which kinds of same-user process hold the project view open (fd, mapping, cwd).
    roots = {os.fsencode(str(view)), os.fsencode(str(view.resolve()))}
    def inside(path: bytes) -> bool:
        return any(path == root or path.startswith(root + b"/") for root in roots)
    def ancestry(pid: int) -> bool:
        for _ in range(64):
            if pid == calcifer: return True
            try: pid = int(Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[1])
            except (OSError, ValueError, IndexError): return False
            if pid <= 1: return False
        return False
    counts = {"holders_calcifer": 0, "holders_calcifer_tree": 0, "holders_chrome": 0, "holders_other": 0, "holding_fd": 0, "holding_map": 0, "holding_cwd": 0}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit(): continue
        pid, kinds = int(entry.name), set()
        try:
            if inside(os.fsencode(os.readlink(entry / "cwd"))): kinds.add("cwd")
            for fd in (entry / "fd").iterdir():
                try:
                    if inside(os.fsencode(os.readlink(fd))): kinds.add("fd"); break
                except OSError: pass
            for line in (entry / "maps").read_bytes().splitlines():
                parts = line.split(maxsplit=5)
                if len(parts) == 6 and inside(parts[5].strip()): kinds.add("map"); break
        except OSError:
            continue
        if not kinds: continue
        for kind in kinds: counts["holding_" + kind] += 1
        role = "calcifer" if pid == calcifer else "chrome" if pid in chrome else "calcifer_tree" if ancestry(pid) else "other"
        counts["holders_" + role] += 1
    return counts


def observe_ready(proof: Proof, project: str, seen: dict[str, int]) -> Observed:
    # Keeps only closed readiness facts for a sanitized failure receipt.
    try:
        value = proof.exercise(project, "ready")
    except Failure:
        seen["ready_errors"] += 1
        raise
    cards = value.get("cards")
    seen.update(last_page_ready=value.get("page_ready") is True, last_cards=cards if type(cards) is int else -1)
    return value


# calcifer-access.md's refusals of a request from this account: what it adds to a native request, whether it carries the bearer,
# and the refusal it gets. A browser's word outranks the bearer; another site's or port's page never says `same-origin`.
REFUSALS: dict[str, tuple[dict[str, str], bool, int, str, str]] = {
    "cross_site": ({"Sec-Fetch-Site": "cross-site", "Sec-Fetch-Mode": "no-cors"}, True, 403, "not_own_page", "not_own_page_refusal"),
    "same_site": ({"Sec-Fetch-Site": "same-site", "Sec-Fetch-Mode": "no-cors"}, True, 403, "not_own_page", "not_own_page_refusal"),
    "none_not_navigation": ({"Sec-Fetch-Site": "none", "Sec-Fetch-Mode": "cors"}, True, 403, "not_own_page", "not_own_page_refusal"),
    # A URL typed or opened by another program, an agent's browser among them: the page opens, /v1 doesn't.
    "none_navigation": ({"Sec-Fetch-Site": "none", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "document"}, False, 403, "not_own_page", "not_own_page_refusal"),
    "foreign_origin": ({"Origin": "http://cozy-foreign.invalid"}, True, 403, "origin_not_allowed", "csrf_no_store_refusal"),
    "no_credential": ({}, False, 401, "unauthenticated", "native_auth_required"),
}


def access_rule(native: Native, receipt: Receipt, project: Project, hmr: bool) -> None:
    # A project route and a media file route: each answers this computer's cozy, and refuses everything else by its code.
    routes = {"project": project_route(project["id"]), "media": project_route(project["id"], "/" + project["text_file"])}
    holder = native.holder()
    upgrade = upgrade_headers(holder["address"])
    facts: dict[str, bool] = {}
    failures: list[str] = []
    def note(case: str, passed: bool, failure: str) -> None:
        facts[case] = passed
        if not passed:
            failures.append(failure)
    for route, path in routes.items():
        note("native_" + route, native.raw("GET", path).status == 200, "native_http_status")
        for case, (extra, bearer, status, code, failure) in REFUSALS.items():
            note(case + "_" + route, native.raw("GET", path, auth=bearer, extra=extra).refused(status, code), failure)
        # A WebSocket's exact Origin counts as the page on GET /__cozy_hmr alone; anywhere else it needs the bearer.
        note("upgrade_" + route, native.raw("GET", path, auth=False, extra=upgrade).refused(401, "unauthenticated"), "upgrade_needs_bearer")
    if hmr:
        # This account's page opens its hot reload socket by its exact Origin, with no bearer.
        note("hmr_own_page", native.raw("GET", HMR, auth=False, extra=upgrade).status == 101, "hmr_websocket_page")
    # Another Unix account, made for this: Calcifer's exact Host, the bearer (stdin only) on the routes, the page's upgrade on HMR.
    asks: dict[str, dict[str, object]] = {route: {"path": path, "headers": {"cozy-project-storage": "3", "Cozy-Client": "cli"}, "bearer": True} for route, path in routes.items()}
    if hmr:
        asks["hmr"] = {"path": HMR, "headers": upgrade, "bearer": False}
    other = OtherAccount()
    try:
        answers = other.ask(holder["address"], holder["token"], list(asks.values()))
    finally:
        facts["other_account_removed"] = other.remove()
    for name, answer in zip(asks, answers):
        refused = answer.get("status") == 403 and answer.get("code") == "other_user" and answer.get("no_store") is True
        note("other_user_" + name, refused, "hmr_websocket_refusal" if name == "hmr" else "other_user_refusal")
    receipt.add("access_rule", **facts)
    require(not failures, failures[0] if failures else "fixture_failure")
    require(facts["other_account_removed"], "other_user_account")


def development_ignored(native: Native, receipt: Receipt) -> None:
    # Vite is up and named, but without `dev = true` Calcifer serves its embedded page and opens no hot reload socket.
    page = native.raw("GET", "/", auth=False)
    embedded = page.status == 200 and page.headers.get("content-type", "").startswith("text/html") and b"/@vite/client" not in page.body
    upgraded = native.raw("GET", HMR, auth=False, extra=upgrade_headers(native.holder()["address"])).status == 101
    receipt.add("development_web_needs_dev", embedded_page=embedded, hmr_upgraded=upgraded)
    require(embedded and not upgraded, "development_web_without_dev")


def scan_case(scanner: Scanner, receipt: Receipt, case: str, roots: list[Path], exclusions: list[Path], located: dict[str, Path]) -> None:
    result = scanner.scan_privileged(roots, exclusions, receipt.path, located)
    if result is None:
        raise Failure("scan_process")
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
    located = places(Path.home(), Path(os.environ["RUNNER_TEMP"]), projects=root, calcifer_vaults=Path.home() / ".cozy/calcifer/vaults")
    proof: Proof | None = None
    foreign: ForeignBrowsers | None = None
    development: Development | None = None
    projects: list[Project] = []
    readiness = {"slot": -1, "ready_errors": 0, "last_page_ready": False, "last_cards": -1}
    editing: dict[str, bool | int] = {}
    phase = "start"
    try:
        if ready.get("development_web") is True:
            phase = "development_start"
            development = Development()
            development.start()
            # Before any project or browser: a Calcifer with `[calcifer] development_web` and no top-level `dev = true` ignores it.
            phase = "development_needs_dev"
            proof = Proof(cli)
            first_pid = proof.process.pid
            until(lambda: native.holder(), lambda holder: holder.get("pid") == first_pid, "holder_ready")
            development_ignored(native, receipt)
            native.call("POST", "/v1/down", {})
            proof.process.wait(timeout=90); proof.close_channel(); proof = None
            development.enable()
            phase = "development_start"
        proof = Proof(cli)
        caf_pid = proof.process.pid
        until(lambda: native.holder(), lambda holder: holder.get("pid") == caf_pid, "holder_ready")
        receipt.add("native_capability", accepted=native.call("GET", "/v1/project-storage").get("version") == 3)
        for slot in ("a", "b"):
            phase = "create_" + slot
            credential = "proof-" + secrets.token_hex(32)
            scanner.add(credential)
            folder = root / slot
            opened = cast(Created, native.document("new", str(folder), "--passphrase-file", "-", stdin=(credential + "\n").encode()))  # cast: new's JSON
            require(isinstance(opened.get("id"), str), "encrypted_create_identity")
            if opened.get("recovery_secret"):
                scanner.add(opened["recovery_secret"])
            vault = cast(VaultStatus, native.document("vault", "status", cwd=folder))  # cast: vault status's JSON
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
            # Project files are served only by content address (<sha256>.<ext>).
            text_file = "files/" + sha(marker.encode()) + ".txt"
            (view / text_file).write_text(marker)
            image = new_png(scanner, marker)
            digest = "sha256:" + sha(image)
            # The content hash itself is derived project data and must not become global custody.
            scanner.add(digest); scanner.add(digest.removeprefix("sha256:"))
            base = project_route(opened["id"])
            native.call("POST", base + "/assets", {"name": asset, "exact": True, "fields": {"text": marker}, "at": {"x": 0, "y": 0, "w": 320}})
            native.call("PUT", "/v1/project-files/" + digest + "?project=" + quote(opened["id"], safe=""), image)
            imported = cast(Imported, native.call("POST", base + "/imports", {"source": digest, "name": picture_asset, "exact": True, "at": {"x": 400, "y": 0, "w": 320}}))  # cast: the import's JSON
            projects.append({"folder": folder, "view": view, "id": opened["id"], "credential": credential, "marker": marker, "edit": edited, "asset": asset, "path": imported["path"], "digest": digest, "text_file": text_file})
        a, b = projects
        if ready.get("native_execution") is True:
            phase = "managed_execution"
            qualify_native(native, scanner, a, receipt)
        phase = "browser_open"
        foreign = ForeignBrowsers()
        for index, project in enumerate(projects):
            native.document("open", cwd=project["folder"])
            readiness.update(slot=index, ready_errors=0, last_page_ready=False, last_cards=-1)
            page = until(partial(observe_ready, proof, project["id"], readiness), lambda value: value.get("page_ready") and value.get("cards", 0) >= 2, "owned_page_ready")
            require(page.get("owned_injected") is False, "owned_page_injected")
        snapshot = proof.ask("snapshot")
        require(snapshot["present"] and snapshot["pages"] == 2 and len(snapshot.get("processes", [])) >= 2 and snapshot["birth"] in snapshot["processes"], "shared_owned_browser")
        browser_birth, profile = snapshot["birth"], Path(snapshot["profile"])
        require(profile.name.startswith("cozy-private-") and tmpfs(profile) and profile.stat().st_mode & 0o777 == 0o700 and profile.stat().st_uid == os.geteuid(), "owned_profile_volatile")
        # Chrome's setproctitle rewrites its argv as one space-joined title.
        title = Path(f"/proc/{browser_birth['pid']}/cmdline").read_bytes()
        argv = set(re.split(rb"[\0 ]", title))
        require(b"--incognito" in argv and b"--remote-debugging-pipe" in argv and ("--user-data-dir=" + str(profile)).encode() in argv and not any(word.startswith(b"--remote-debugging-port") for word in argv), "actual_private_browser_flags")
        launched = launch_environments(snapshot["processes"])
        require(launched and tmpdir_singleton(profile) and all(key in environment and Path(os.fsdecode(environment[key])).is_relative_to(profile) for environment in launched for key in REDIRECTS), "actual_private_browser_redirects")
        require(not any(marker in title for marker in scanner.markers), "private_browser_argv_canary")
        require(all(browser_birth != held[1] for held in foreign.held), "browser_ownership_distinct")
        receipt.add("shared_owned_browser", pages=2, volatile=True, unrelated=foreign.preserved(), birth_sha256=sha(json.dumps(browser_birth, sort_keys=True).encode()),
                    title_flags=True, tmpdir_singleton=True, launch_environment_redirected=True, launch_environments=len(launched), nothing_injected=True)
        if ready.get("remote_cpu") is True:
            phase = "remote_cpu"
            qualify_remote(native, a, receipt, ready)
        # The page sends no credential: its capability names this account and this Calcifer's birth, as the native CLI's does.
        account, calcifer_birth = principal(), daemon_birth(proof.process.pid)
        for project in projects:
            phase = "headers"
            base = project_route(project["id"])
            paths = [base + "/" + project["text_file"], base + "/" + project["path"], base + "/thumbs/" + project["digest"].removeprefix("sha256:") + "?w=128"]
            for number, path in enumerate(paths):
                observed = proof.exercise(project["id"], "media", path=path)
                facts = {"admitted": observed.get("admitted") is True, "principal": observed.get("principal") == account, "daemon_birth": observed.get("daemon_birth") == calcifer_birth}
                bound = all(facts.values())
                served = bound and observed.get("status") == 200 and observed.get("no_store")
                if not served:
                    status = observed.get("status")
                    receipt.add("owned_media_failure", index=number, status=status if type(status) is int else -1, no_store=observed.get("no_store") is True, **facts)
                require(bound, "owned_media_capability")
                require(served, "owned_no_store_200")
                if number == 1:
                    ranged = proof.exercise(project["id"], "media", path=path, range=True)
                    conditional = proof.exercise(project["id"], "media", path=path, conditional=True)
                    require(ranged.get("status") == 206 and ranged.get("no_store") and ranged.get("range"), "owned_no_store_206")
                    require(conditional.get("status") in {200, 304} and conditional.get("no_store"), "owned_no_store_conditional")
                    receipt.add("media_" + ("a" if project is a else "b"), status=200, range_status=ranged["status"], conditional_status=conditional["status"], body_sha256=observed["body_sha256"],
                                capability_bound=True, authorization_sent=False)
            storage = proof.exercise(project["id"], "storage")
            require(storage == {"project_preferences": 0, "stored_credential": False, "indexed_databases": 0, "cache_stores": 0, "cookies": 0}, "browser_durable_storage")
        # A private name in a refused path and Origin: refused, no-store, and never found outside by the scans.
        refused = native.raw("GET", project_route(a["id"], "/files/" + a["marker"]), extra={"Origin": "http://" + a["marker"] + ".invalid"})
        require(refused.refused(403, "origin_not_allowed"), "csrf_no_store_refusal")
        access_rule(native, receipt, a, development is not None)
        scan_case(scanner, receipt, "live_outside_scan", roots, [a["view"], b["view"], profile], located)
        if development is not None:
            phase = "development_hmr"
            development.no_api_bridge()
            initial = proof.exercise(b["id"], "hmr_start")
            require(initial.get("development") and initial.get("value") == 1, "owned_hmr_initial_module")
            require(initial.get("csp_nonce") and initial.get("refresh_nonce") and initial.get("style_nonce"), "owned_hmr_nonce_missing")
            headers = native.raw("GET", "/").headers
            policy = headers.get("content-security-policy", "")
            require("'nonce-" in policy and "'unsafe-inline'" not in policy and "ws://" + native.holder()["address"] in policy and "no-store" in headers.get("cache-control", ""), "owned_hmr_csp_header")
            development.update()
            updated = until(partial(proof.exercise, b["id"], "hmr_observe"), lambda value: value.get("value") == 2, "owned_hmr_update")
            require(updated.get("document_sha256") == initial["document_sha256"], "owned_hmr_reloaded_document")
            refused_dev = native.raw("GET", "/files/" + a["marker"])
            require(refused_dev.status == 404, "development_private_path_forwarded")
            receipt.add("owned_development_hmr", same_origin=True, actual_update=True, document_retained=True, csp_nonce=True, refresh_nonce=True, style_nonce=True, private_path_refused=True, direct_vite_api_refused=True)
        phase = "queued_b_edit"
        held = sqlite3.connect(b["view"] / "project.db", timeout=1)
        held.execute("BEGIN IMMEDIATE")
        locking = None
        try:
            def edit_b(proof: Proof = proof) -> Observed:
                value = proof.exercise(b["id"], "edit", asset=b["asset"], field="text", value=b["edit"])
                editing.clear()
                editing.update({key: item for key, item in value.items() if key in EDIT_FACTS and (type(item) is bool or type(item) is int)})
                return value
            until(edit_b, lambda value: value.get("typed"), "real_browser_edit")
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
        if locking is None:
            raise Failure("lock_not_started")
        stdout, stderr = locking.communicate(timeout=90)
        busy = locking.returncode != 0
        if busy:
            refusal = json.loads(stdout)
            present = proof.ask("snapshot")
            receipt.add("a_lock_busy", browser_present=present.get("present") is True, **busy_facts(refusal),
                        **view_holders(a["view"], proof.process.pid, {process["pid"] for process in snapshot["processes"]}))
            require(refusal.get("error", {}).get("code") == "project_busy", "a_lock_unexpected_refusal")
            require(proof.exercise(b["id"], "observe", needle=b["edit"]).get("visible"), "busy_lock_lost_b_draft")
            # An orderly busy refusal is valid. Finish the same real UI edit, then retry A's lock.
            until(partial(proof.exercise, b["id"], "edit", asset=b["asset"], field="text", value=b["edit"]), lambda value: value.get("typed"), "busy_edit_resume")
            until(lambda: cast(AssetAnswer, native.call("GET", project_route(b["id"], "/assets/" + quote(b["asset"], safe=""))))["asset"]["fields"]["text"], lambda value: value == b["edit"], "b_edit_not_persisted")  # cast: the asset's JSON
            stdout = native.command("vault", "lock", cwd=a["folder"])
        require(json.loads(stdout)["vault"]["state"] == "locked", "a_lock_after_b_saved")
        saved = cast(AssetAnswer, native.call("GET", project_route(b["id"], "/assets/" + quote(b["asset"], safe=""))))  # cast: the asset's JSON
        require(saved["asset"]["fields"]["text"] == b["edit"], "b_edit_not_persisted")
        until(lambda: [alive(process) for process in snapshot["processes"]], lambda live: not any(live), "owned_browser_tree_not_retired")
        require(not profile.exists() and foreign.preserved() and not (a["view"] / "root-data.txt").exists(), "whole_browser_retirement")
        receipt.add("queued_b_edit_then_a_lock", saved=True, owned_retired=True, profile_removed=True, unrelated_preserved=True, busy_refusal=busy)
        scan_case(scanner, receipt, "a_locked_outside_scan", roots, [b["view"]], located)
        phase = "last_window_close"
        native.document("open", cwd=b["folder"])
        until(partial(proof.exercise, b["id"], "ready"), lambda value: value.get("page_ready"), "reopened_page_ready")
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
        until(partial(proof.exercise, b["id"], "ready"), lambda value: value.get("page_ready"), "crash_page_ready")
        crashed = proof.ask("snapshot")
        kill_exact(crashed["birth"])
        until(lambda: [alive(process) for process in crashed["processes"]], lambda live: not any(live), "crashed_browser_tree_not_retired")
        until(lambda: Path(crashed["profile"]).exists(), lambda exists: not exists, "crashed_profile_not_retired")
        require(foreign.preserved(), "foreign_browser_changed")
        receipt.add("browser_crash", process_gone=not alive(crashed["birth"]), profile_removed=True, unrelated_preserved=True)
        phase = "caf_crash"
        native.document("open", cwd=b["folder"])
        until(partial(proof.exercise, b["id"], "ready"), lambda value: value.get("page_ready"), "caf_crash_page_ready")
        orphaned = proof.ask("snapshot")
        previous = native.holder()
        # SIGKILL cannot run Caf's Drop, vault close, or the live browser watcher.
        # Only the next real Caf holder may recover its nonsecret tmpfs ownership receipt.
        kill_exact(proof.birth)
        proof.process.wait(timeout=90); proof.close_channel(); proof = None
        proof = Proof(cli)
        caf_pid = proof.process.pid
        until(lambda: native.holder(), lambda value: value.get("pid") == caf_pid, "restart_holder_ready")
        rejected = native.raw("GET", "/v1/project-storage", extra={"Authorization": "Bearer " + previous["token"]}, preflight=False)
        require(rejected.refused(401, "unauthenticated"), "old_holder_authority_survived")
        until(lambda: [alive(process) for process in orphaned["processes"]], lambda live: not any(live), "caf_crash_browser_tree_not_retired")
        until(lambda: Path(orphaned["profile"]).exists(), lambda exists: not exists, "caf_crash_profile_not_retired")
        require(foreign.preserved(), "foreign_browser_changed")
        receipt.add("caf_crash_recovery", process_gone=True, profile_removed=True, old_authority_refused=True, unrelated_preserved=True)
        phase = "cold_restart"
        native.document("vault", "lock", cwd=b["folder"])
        previous = native.holder()
        native.call("POST", "/v1/down", {})
        proof.process.wait(timeout=90); proof.close_channel(); proof = None
        proof = Proof(cli)
        until(lambda: native.holder(), lambda value: value.get("pid") == proof.process.pid, "restart_holder_ready")
        rejected = native.raw("GET", "/v1/project-storage", extra={"Authorization": "Bearer " + previous["token"]}, preflight=False)
        require(rejected.refused(401, "unauthenticated"), "old_holder_authority_survived")
        native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=b["folder"], stdin=(b["credential"] + "\n").encode())
        native.document("open", cwd=b["folder"])
        until(lambda: proof.exercise(b["id"], "ready"), lambda value: value.get("page_ready"), "cold_page_ready")
        fresh = proof.ask("snapshot")
        require(fresh["birth"] not in (browser_birth, last["birth"], crashed["birth"]), "browser_birth_reused")
        if a.get("native_run_id"):
            native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=a["folder"], stdin=(a["credential"] + "\n").encode())
            reopened = cast(VaultStatus, native.document("vault", "status", cwd=a["folder"]))  # cast: vault status's JSON
            require(sha((Path(reopened["view"]) / "native-allowed.txt").read_bytes()) == a["native_allowed_sha256"], "native_file_not_retained_after_restart")
            retained = cast(RunDetail, native.call("GET", project_route(a["id"], "/runs/" + quote(a["native_run_id"], safe=""))))  # cast: the run's JSON
            require(retained.get("status") == "completed" and retained.get("result", {}).get("allowed_sha256") == a["native_allowed_sha256"], "native_history_not_retained_after_restart")
            native.document("vault", "lock", cwd=a["folder"])
            receipt.add("native_lock_restart_retention", file_retained=True, history_retained=True, output_sha256=a["native_allowed_sha256"])
        native.document("vault", "lock", cwd=b["folder"])
        receipt.add("cold_restart", new_authority=True, new_browser=True, unrelated_preserved=foreign.preserved())
        scan_case(scanner, receipt, "all_locked_outside_scan", roots, [], located)
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
        if isinstance(error, Failure):
            receipt.value["failed_check"] = error.code
        receipt.save()
        if native.failure is not None:
            try:
                receipt.add("cli_failure", **native.failure)
            except Failure:
                receipt.value["diagnostic_rejected"] = True
        if editing:
            receipt.add("browser_edit_observation", **editing)
        if phase == "browser_open" and proof is not None and alive(proof.birth):
            try:
                observed = proof.ask("snapshot")
                pages = observed.get("pages")
                receipt.add("browser_open_failure", present=observed.get("present") is True,
                            retirement_incomplete=observed.get("retirement_incomplete") is True,
                            caf_alive=alive(proof.birth), observed_processes=len(observed.get("processes", [])),
                            pages=pages if type(pages) is int else -1, **readiness)
            except BaseException:
                receipt.add("browser_open_failure", snapshot_unavailable=True, caf_alive=alive(proof.birth), **readiness)
            if readiness["slot"] >= 0:
                try:
                    page = proof.exercise(projects[readiness["slot"]]["id"], "diagnose")
                    receipt.add("owned_page_diagnostic", **{key: value for key, value in page.items() if key in PAGE_FACTS and type(value) in (bool, int)})
                except BaseException:
                    receipt.add("owned_page_diagnostic", unavailable=True)
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
            try:
                if development is not None:
                    development.close()
                    receipt.add("development_restoration", restored=True)
            except BaseException:
                receipt.value["development_restore_failed"] = True
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
