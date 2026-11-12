"""Old-first public package publication, before private subjects and before any V8 Calcifer.

Caller owns the hosted bootstrap, login/admin grant and later exact-cohort switch. This helper
never invokes the new candidate, clears history, deletes global caches, or resets the home.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
from urllib.parse import quote
import urllib.request

from common import require, sha

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from test_hub import require_test_hub

PACKAGE = "e2e-containment-legacy"
RELEASE = "0.1.0"
TARGET = PACKAGE + "/echo@" + RELEASE
FILES = ("pyproject.toml", "package.toml", "containment_legacy_probe/__init__.py")


def _call(old_cli: Path, *words: str, cwd: Path | None = None) -> bytes:
    completed = subprocess.run([str(old_cli), "--json", *words], cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    require(completed.returncode == 0, "legacy_public_bootstrap_cli")
    return completed.stdout


def _version() -> int:
    database = Path.home() / ".cozy/calcifer/state.db"
    require(database.is_file() and not database.is_symlink(), "legacy_public_bootstrap_database")
    connection = sqlite3.connect("file:" + quote(str(database), safe="/") + "?mode=ro", uri=True)
    try:
        return connection.execute("PRAGMA user_version").fetchone()[0]
    finally:
        connection.close()


def _runs(old_cli: Path) -> dict[str, dict]:
    listed = json.loads(_call(old_cli, "run", "ls", "--all", "--limit", "0"))
    require(isinstance(listed, dict) and isinstance(listed.get("runs"), list), "legacy_public_bootstrap_run_list")
    return {run["id"]: run for run in listed["runs"] if isinstance(run, dict) and isinstance(run.get("id"), str)}


def publish_old(old_cli: Path, work: Path) -> dict:
    """Return public publication provenance; run only before generating private canaries.

    The old CLI must already be the installed default-home cohort, signed in as the owned
    test Hub's admin. Public publication records remain explicit legitimate maintenance;
    the caller must not confuse them with later private history or discard that history.
    """
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_OS") == "Linux" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "legacy_public_bootstrap_hosted")
    require_test_hub(os.environ.get("COZY_HUB_URL", ""), os.environ.get("COZY_TEST_HUB_ORIGIN", ""))
    installed = Path.home() / ".cozy/bin/cozy"
    with old_cli.open("rb") as source_binary:
        old_sha256 = hashlib.file_digest(source_binary, "sha256").hexdigest()
    with installed.open("rb") as installed_binary:
        require(hashlib.file_digest(installed_binary, "sha256").hexdigest() == old_sha256, "legacy_public_bootstrap_installed_cohort")
    version = subprocess.run([str(old_cli), "--version"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    require(version.returncode == 0 and version.stdout.strip() == b"cozy 0.24.0", "legacy_public_bootstrap_version")
    # This can start only the already-verified installed OLD cohort, never the candidate.
    account = json.loads(_call(old_cli, "whoami"))
    require(isinstance(account.get("account"), str) and account["account"], "legacy_public_bootstrap_account")
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_bytes())
    require(holder.get("version") == "0.24.0" and _version() == 7, "legacy_public_bootstrap_v7")
    with Path(f"/proc/{holder['pid']}/exe").open("rb") as executable:
        require(hashlib.file_digest(executable, "sha256").hexdigest() == old_sha256, "legacy_public_bootstrap_old_holder")
    source = Path(__file__).resolve().with_name("legacy-package")
    work.mkdir(parents=True, exist_ok=True)
    copied = Path(tempfile.mkdtemp(prefix="public-legacy-package-", dir=work))
    declared = []
    for relative in FILES:
        path = source / relative
        require(path.is_file() and not path.is_symlink(), "legacy_public_bootstrap_static_source")
        body = path.read_bytes()
        target = copied / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as output:
            output.write(body)
        declared.append([relative, sha(body)])
    for ancestor in [copied, *copied.parents]:
        require(not any((ancestor / marker).exists() for marker in ("project.db", "project.vault.keys.json")), "legacy_public_bootstrap_source_is_project")
    before = _runs(old_cli)
    _call(old_cli, "package", "publish", "--root", "-y", cwd=copied)
    require(_version() == 7, "legacy_public_bootstrap_schema_changed")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    endpoint = os.environ["COZY_HUB_URL"].rstrip("/") + "/v1/packages/_/" + PACKAGE + "/releases/" + RELEASE
    with opener.open(endpoint, timeout=60) as response:
        require(response.status == 200, "legacy_public_bootstrap_release_absent")
        details = json.loads(response.read(2 << 20))
    interface = details.get("package_interface")
    require(details.get("release", {}).get("release") == RELEASE and isinstance(interface, dict) and b'"echo"' in json.dumps(interface, separators=(",", ":")).encode(), "legacy_public_bootstrap_interface")
    after = _runs(old_cli)
    maintenance = []
    for identifier in sorted(after.keys() - before.keys()):
        run = after[identifier]
        require(not run.get("project_id") and not run.get("project") and str(run.get("target", "")).startswith("cozy/"), "legacy_public_bootstrap_unproven_run")
        maintenance.append(identifier)
    return {
        "target": TARGET,
        "package": PACKAGE,
        "release": RELEASE,
        "old_binary_sha256": old_sha256,
        "source_sha256": sha(json.dumps(declared, separators=(",", ":")).encode()),
        "schema_version": 7,
        "package_interface_sha256": sha(json.dumps(interface, sort_keys=True, separators=(",", ":")).encode()),
        "public_maintenance_run_ids": maintenance,
    }
