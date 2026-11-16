# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=11", "python-snappy>=0.7"]
# ///
"""Hosted only: real installed CLI and Calcifer, with private values retained only in RAM.

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
import sys
from typing import NotRequired, TypedDict, cast
from urllib.parse import quote

from common import (HMR, Calcifer, Failure, Native, OtherAccount, Project, Receipt, Reply, RunDetail, VaultStatus, alive, birth,
                    daemon_birth, kill_exact, principal, project_route, require, sha, until, upgrade_headers)
from scan import Scanner, places
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


# What Calcifer's page sends with each of its requests: no bearer, and the browser's word that it's the page's own.
PAGE = {"Sec-Fetch-Site": "same-origin", "Sec-Fetch-Mode": "cors", "Sec-Fetch-Dest": "empty"}


def as_page(native: Native, path: str, extra: dict[str, str] | None = None) -> Reply:
    return native.raw("GET", path, auth=False, extra={**PAGE, **(extra or {})})


def page_headers(native: Native, receipt: Receipt, project: Project, name: str) -> None:
    """What the page gets of an encrypted project, as any browser fetches it: never stored (200, 206, conditional), and its capability
    names this account and this Calcifer."""
    capability = as_page(native, "/v1/project-storage").json()
    require(capability.get("principal") == principal() and capability.get("daemon_birth") == daemon_birth(native.holder()["pid"]), "page_capability")
    base = project_route(project["id"])
    paths = [base + "/" + project["text_file"], base + "/" + project["path"], base + "/thumbs/" + project["digest"].removeprefix("sha256:") + "?w=128"]
    for number, path in enumerate(paths):
        # A thumb not drawn yet is `202 thumb_pending` (cl-982): no picture and no error, asked again until it is.
        served = until(lambda: as_page(native, path), lambda reply: reply.status != 202, "page_no_store_200")
        require(served.status == 200 and "no-store" in served.headers.get("cache-control", ""), "page_no_store_200")
        if number == 1:
            ranged = as_page(native, path, {"Range": "bytes=0-15"})
            require(ranged.status == 206 and "no-store" in ranged.headers.get("cache-control", ""), "page_no_store_206")
            conditional = as_page(native, path, {"If-None-Match": served.headers.get("etag", '"none"')})
            require(conditional.status in {200, 304} and "no-store" in conditional.headers.get("cache-control", ""), "page_no_store_conditional")
            receipt.add("media_" + name, status=200, range_status=ranged.status, conditional_status=conditional.status, body_sha256=sha(served.body),
                        capability_bound=True, authorization_sent=False)


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
    calcifer: Calcifer | None = None
    development: Development | None = None
    projects: list[Project] = []
    phase = "start"
    try:
        if ready.get("development_web") is True:
            phase = "development_start"
            development = Development()
            development.start()
            # Before any project or browser: a Calcifer with `[calcifer] development_web` and no top-level `dev = true` ignores it.
            phase = "development_needs_dev"
            calcifer = Calcifer(cli)
            first_pid = calcifer.process.pid
            until(lambda: native.holder(), lambda holder: holder.get("pid") == first_pid, "holder_ready")
            development_ignored(native, receipt)
            native.call("POST", "/v1/down", {})
            calcifer.process.wait(timeout=90); calcifer = None
            development.enable()
            phase = "development_start"
        calcifer = Calcifer(cli)
        caf_pid = calcifer.process.pid
        until(lambda: native.holder(), lambda holder: holder.get("pid") == caf_pid, "holder_ready")
        receipt.add("native_capability", accepted=native.call("GET", "/v1/project-storage").get("version") == 3)
        for slot in ("a", "b"):
            phase = "create_" + slot
            credential = "proof-" + secrets.token_hex(32)
            scanner.add(credential)
            # Its name is its folder's, unique, so finding it anywhere but in that folder's path is finding a trace (cl-984).
            folder = root / (slot + "-" + secrets.token_hex(12))
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
        if ready.get("remote_cpu") is True:
            phase = "remote_cpu"
            qualify_remote(native, a, receipt, ready)
        # Any browser of this account opens the page; what it gets of an encrypted project is never stored, whichever browser asks.
        for project in projects:
            phase = "headers"
            page_headers(native, receipt, project, "a" if project is a else "b")
        # A private name in a refused path and Origin: refused, no-store, and never found outside by the scans.
        refused = native.raw("GET", project_route(a["id"], "/files/" + a["marker"]), extra={"Origin": "http://" + a["marker"] + ".invalid"})
        require(refused.refused(403, "origin_not_allowed"), "csrf_no_store_refusal")
        access_rule(native, receipt, a, development is not None)
        scan_case(scanner, receipt, "live_outside_scan", roots, [a["view"], b["view"]], located)
        if development is not None:
            phase = "development_headers"
            development.no_api_bridge()
            headers = native.raw("GET", "/").headers
            policy = headers.get("content-security-policy", "")
            require("'nonce-" in policy and "'unsafe-inline'" not in policy and "ws://" + native.holder()["address"] in policy and "no-store" in headers.get("cache-control", ""), "development_csp_header")
            refused_dev = native.raw("GET", "/files/" + a["marker"])
            require(refused_dev.status == 404, "development_private_path_forwarded")
            receipt.add("development_headers", csp_nonce=True, private_path_refused=True, direct_vite_api_refused=True)
        phase = "a_lock"
        locked = native.document("vault", "lock", cwd=a["folder"])
        require(cast(dict[str, dict[str, object]], locked)["vault"]["state"] == "locked" and not (a["view"] / "root-data.txt").exists(), "a_lock")  # cast: vault lock's JSON
        receipt.add("a_lock", locked=True)
        # Paul's invariant (cl-984): once locked, nothing of a project stays outside its folder but that folder's path: not its id,
        # its name, its prompts, inputs, outputs, runs or logs.
        scanner.identity(a["folder"], a["id"], a["folder"].name)
        scan_case(scanner, receipt, "a_locked_outside_scan", roots, [b["view"]], located)
        phase = "caf_crash"
        previous = native.holder()
        # SIGKILL runs nothing of Calcifer's: the next one starts on what it left.
        kill_exact(calcifer.birth)
        calcifer.process.wait(timeout=90); calcifer = None
        calcifer = Calcifer(cli)
        caf_pid = calcifer.process.pid
        until(lambda: native.holder(), lambda value: value.get("pid") == caf_pid, "restart_holder_ready")
        rejected = native.raw("GET", "/v1/project-storage", extra={"Authorization": "Bearer " + previous["token"]}, preflight=False)
        require(rejected.refused(401, "unauthenticated"), "old_holder_authority_survived")
        receipt.add("caf_crash_recovery", old_authority_refused=True)
        phase = "cold_restart"
        native.document("vault", "lock", cwd=b["folder"])
        previous = native.holder()
        native.call("POST", "/v1/down", {})
        calcifer.process.wait(timeout=90)
        calcifer = Calcifer(cli)
        caf_pid = calcifer.process.pid
        until(lambda: native.holder(), lambda value: value.get("pid") == caf_pid, "restart_holder_ready")
        rejected = native.raw("GET", "/v1/project-storage", extra={"Authorization": "Bearer " + previous["token"]}, preflight=False)
        require(rejected.refused(401, "unauthenticated"), "old_holder_authority_survived")
        native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=b["folder"], stdin=(b["credential"] + "\n").encode())
        if a.get("native_run_id"):
            native.document("vault", "unlock", "--passphrase-file", "-", "--idle", "0", cwd=a["folder"], stdin=(a["credential"] + "\n").encode())
            reopened = cast(VaultStatus, native.document("vault", "status", cwd=a["folder"]))  # cast: vault status's JSON
            require(sha((Path(reopened["view"]) / "native-allowed.txt").read_bytes()) == a["native_allowed_sha256"], "native_file_not_retained_after_restart")
            retained = cast(RunDetail, native.call("GET", project_route(a["id"], "/runs/" + quote(a["native_run_id"], safe=""))))  # cast: the run's JSON
            require(retained.get("status") == "completed" and retained.get("result", {}).get("allowed_sha256") == a["native_allowed_sha256"], "native_history_not_retained_after_restart")
            native.document("vault", "lock", cwd=a["folder"])
            receipt.add("native_lock_restart_retention", file_retained=True, history_retained=True, output_sha256=a["native_allowed_sha256"])
        native.document("vault", "lock", cwd=b["folder"])
        receipt.add("cold_restart", new_authority=True)
        scanner.identity(b["folder"], b["id"], b["folder"].name)
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
        receipt.save()
        return 1
    finally:
        if calcifer is not None and alive(calcifer.birth):
            try:
                for project in projects:
                    native.command("vault", "lock", cwd=project["folder"])
                native.call("POST", "/v1/down", {})
                calcifer.process.wait(timeout=30)
            except BaseException:
                receipt.value["cleanup_incomplete"] = True
                receipt.save()
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
