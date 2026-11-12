"""Actual CLI execution and Cozy-managed project storage, not an arbitrary-code sandbox."""
from __future__ import annotations

import json
from pathlib import Path
import secrets
import subprocess

from common import Native, Receipt, require, sha
from scan import Scanner


PROGRAM = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import hashlib
import os
from pathlib import Path
import resource
import sys
import tempfile
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
SOURCE = SOURCE_MARKER
class Request(msgspec.Struct, forbid_unknown_fields=True):
    marker: str
    allowed: str
class Answer(msgspec.Struct):
    report: File
    allowed_sha256: str
    temporary: str
    cache: str
    home: str
    working_directory: str
    runtime_prefix: str
    source_path: str
    core_zero: bool
@app.entrypoint(accelerator=False)
def storage(payload: Request, out: Outputs) -> Answer:
    content = payload.marker + "\\n" + SOURCE
    allowed = Path(payload.allowed)
    allowed.write_text(content)
    with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as stream:
        stream.write(content)
        temporary = stream.name
    cache = Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "managed-storage-proof"
    cache.mkdir(parents=True, exist_ok=True)
    (cache / "entry.txt").write_text(content)
    report = out.temporary_file(".txt")
    report.write_text(content)
    return Answer(out.save_file(report, media_type="text/plain"), hashlib.sha256(allowed.read_bytes()).hexdigest(), temporary, str(cache), str(Path.home()), str(Path.cwd()), sys.prefix, __file__, resource.getrlimit(resource.RLIMIT_CORE) == (0, 0))
'''


def _documents(body: bytes) -> list[dict]:
    try:
        value = json.loads(body)
        if isinstance(value, dict):
            return [value]
    except (ValueError, UnicodeDecodeError):
        pass
    result = []
    for line in body.splitlines():
        try:
            value = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        if isinstance(value, dict):
            result.append(value)
    return result


def qualify(native: Native, scanner: Scanner, project: dict, receipt: Receipt) -> None:
    view = project["view"]
    folder = view / "managed-execution-proof"
    folder.mkdir(mode=0o700)
    source_marker = "MANAGED_SOURCE_" + secrets.token_hex(24)
    payload_marker = "MANAGED_PAYLOAD_" + secrets.token_hex(24)
    scanner.add(source_marker); scanner.add(payload_marker)
    script = folder / "probe.py"
    script.write_text(PROGRAM.replace("SOURCE_MARKER", repr(source_marker)))
    scanner.add(sha(script.read_bytes())); scanner.add("sha256:" + sha(script.read_bytes()))
    allowed = view / "native-allowed.txt"
    request = folder / "input.json"
    request.write_text(json.dumps({"marker": payload_marker, "allowed": str(allowed)}))
    require(not allowed.exists(), "native_allowed_preexists")
    # Private values travel as a project-owned input file, never argv or environment.
    completed = subprocess.run([str(native.cli), "--json", "run", str(script) + "/storage", "--input", str(request), "--on", "local"], cwd=project["folder"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    documents = _documents(completed.stdout)
    runs = [document.get("run", document) for document in documents]
    errors = [document["error"] for document in documents if isinstance(document.get("error"), dict)]
    errors += [run["reason"] for run in runs if isinstance(run, dict) and isinstance(run.get("reason"), dict)]
    errors += [run["result"]["error"] for run in runs if isinstance(run, dict) and isinstance(run.get("result"), dict) and isinstance(run["result"].get("error"), dict)]
    known_codes = ("project_execution_unavailable", "project_worker_unavailable", "project_migration_pending", "private_storage_unavailable", "project_transport_unsupported", "project_busy", "run_failed", "usage")
    receipt.add("managed_execution_observation", exit_code=completed.returncode, file_created=allowed.exists(), refusal_codes={code: any(error.get("code") == code for error in errors) for code in known_codes})
    require(completed.returncode == 0, "managed_execution_failed")
    records = [run for run in runs if isinstance(run, dict) and isinstance(run.get("result"), dict) and "allowed_sha256" in run["result"]]
    require(records, "native_terminal_result_missing")
    run, result = records[-1], records[-1]["result"]
    expected = (payload_marker + "\n" + source_marker).encode()
    require(allowed.read_bytes() == expected and result["allowed_sha256"] == sha(expected), "native_project_write_failed")
    for field in ("temporary", "cache", "home", "working_directory", "runtime_prefix", "source_path"):
        path = Path(result[field]).resolve()
        require(path.is_relative_to(view.resolve()), "managed_path_outside_project")
    require(Path(result["temporary"]).read_bytes() == expected and (Path(result["cache"]) / "entry.txt").read_bytes() == expected, "managed_cache_contents")
    require(result["core_zero"], "managed_core_limit")
    require(isinstance(run.get("id"), str), "native_private_run_identity_missing")
    for item in runs:
        if isinstance(item, dict) and isinstance(item.get("id"), str):
            scanner.add(item["id"])
    scanner.add(result["allowed_sha256"]); scanner.add("sha256:" + result["allowed_sha256"])
    project["native_run_id"], project["native_allowed_sha256"] = run["id"], result["allowed_sha256"]
    receipt.add("managed_project_execution", actual_cli=True, project_write=True, temporary_private=True, cache_private=True, home_private=True, source_private=True, runtime_private=True, core_limit_zero=True, output_sha256=result["allowed_sha256"])
