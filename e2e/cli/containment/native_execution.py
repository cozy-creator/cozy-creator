"""Actual installed-CLI native confinement, only on a coordinator-authorized hosted VM."""
from __future__ import annotations

import errno
import json
import os
from pathlib import Path
import secrets
import subprocess

from common import Native, Receipt, require, sha
from scan import Scanner


class CorePattern:
    """An explicit hosted-fixture host change, restored even when qualification fails."""
    path = Path("/proc/sys/kernel/core_pattern")

    def __init__(self, ready: dict):
        require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_OS") == "Linux" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "owned_host_core_control_required")
        require(ready.get("allow_host_core_pattern_change") is True, "core_control_not_authorized")
        self.original = self.path.read_bytes().strip()
        self.changed = False

    def set(self, pattern: bytes) -> None:
        require(b"\n" not in pattern and pattern, "core_pattern_invalid")
        completed = subprocess.run(["sudo", "-n", "tee", str(self.path)], input=pattern + b"\n", stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        require(completed.returncode == 0 and self.path.read_bytes().strip() == pattern, "owned_host_core_control_unavailable")
        self.changed = True

    def restore(self) -> None:
        if self.changed:
            self.set(self.original)
            self.changed = False


# This is the existing App/msgspec/Outputs end-user API used by private_execution.py.
# It is real project source, staged and invoked only by the installed candidate CLI.
PROGRAM = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import ctypes
import errno
import hashlib
import os
from pathlib import Path
import resource
import subprocess
import sys
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
SOURCE = SOURCE_MARKER
class Request(msgspec.Struct, forbid_unknown_fields=True):
    marker: str
    allowed: str
    outside_home: str
    outside_tmp: str
class Answer(msgspec.Struct):
    report: File
    allowed_sha256: str
    home_errno: int
    tmp_errno: int
    core_zero: bool
    core_raise_denied: bool
    dump_raise_denied: bool
    abort_signal: int
@app.entrypoint(accelerator=False)
def containment(payload: Request, out: Outputs) -> Answer:
    allowed = Path(payload.allowed)
    allowed.write_text(payload.marker + "\\n" + SOURCE)
    def denied(path):
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except OSError as error:
            return error.errno or -1
        with os.fdopen(descriptor, "w") as stream:
            stream.write(payload.marker)
        return 0
    home = denied(payload.outside_home)
    temp = denied(payload.outside_tmp)
    limits = resource.getrlimit(resource.RLIMIT_CORE)
    raised = False
    try:
        resource.setrlimit(resource.RLIMIT_CORE, (1024 * 1024, 1024 * 1024))
    except (OSError, ValueError):
        raised = True
    libc = ctypes.CDLL(None, use_errno=True)
    ctypes.set_errno(0)
    dumped = libc.prctl(4, 1, 0, 0, 0)  # PR_SET_DUMPABLE, denied by the installed native policy.
    dump_denied = dumped == -1 and ctypes.get_errno() in (errno.EPERM, errno.EACCES)
    child = subprocess.run([sys.executable, "-I", "-c", "import os; os.abort()"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    report = out.temporary_file(".txt")
    report.write_text(payload.marker + "\\n" + SOURCE)
    return Answer(out.save_file(report, media_type="text/plain"), hashlib.sha256(allowed.read_bytes()).hexdigest(), home, temp, limits == (0, 0), raised, dump_denied, -child.returncode)
'''


def _writable_parent(path: Path) -> None:
    """A data-free baseline proves denial is not merely a missing/read-only parent."""
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        identity = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    current = path.lstat()
    require((identity.st_dev, identity.st_ino) == (current.st_dev, current.st_ino), "outside_baseline_replaced")
    path.unlink()


def _run(native: Native, project: dict, script: Path, request: Path) -> subprocess.CompletedProcess:
    # No marker, payload, target canary or credential appears in argv/environment.
    return subprocess.run([str(native.cli), "--json", "run", str(script) + "/containment", "--input", str(request), "--on", "local"], cwd=project["folder"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def _documents(body: bytes) -> list[dict]:
    result = []
    for line in body.splitlines():
        try:
            value = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        if isinstance(value, dict):
            result.append(value)
    return result


def qualify(native: Native, scanner: Scanner, project: dict, receipt: Receipt, core: CorePattern) -> None:
    view = project["view"]
    folder = view / "native-confinement-proof"
    folder.mkdir(mode=0o700)
    source_marker = "NATIVE_SOURCE_" + secrets.token_hex(24)
    payload_marker = "NATIVE_PAYLOAD_" + secrets.token_hex(24)
    scanner.add(source_marker); scanner.add(payload_marker)
    script = folder / "probe.py"
    script.write_text(PROGRAM.replace("SOURCE_MARKER", repr(source_marker)))
    allowed = view / "native-allowed.txt"
    suffix = secrets.token_hex(12)
    outside_home = Path.home() / ("containment-forbidden-" + suffix)
    outside_tmp = Path("/tmp") / ("containment-forbidden-" + suffix)
    _writable_parent(outside_home); _writable_parent(outside_tmp)
    request = folder / "input.json"
    request.write_text(json.dumps({"marker": payload_marker, "allowed": str(allowed), "outside_home": str(outside_home), "outside_tmp": str(outside_tmp)}))
    require(not allowed.exists(), "native_allowed_preexists")
    # This is an actual host setting and an actual CLI request, not a mocked policy probe.
    core.set(b"|/usr/bin/false")
    piped = _run(native, project, script, request)
    documents = _documents(piped.stdout)
    coded = [document["error"] for document in documents if isinstance(document.get("error"), dict)]
    coded += [document["run"]["reason"] for document in documents if isinstance(document.get("run"), dict) and isinstance(document["run"].get("reason"), dict)]
    require(piped.returncode != 0 and not allowed.exists() and any(error.get("code") in {"project_execution_unavailable", "project_transport_unsupported"} for error in coded), "piped_core_not_refused")
    require(b"piped kernel core handler" in piped.stdout + piped.stderr, "piped_core_refusal_not_specific")
    receipt.add("native_piped_core_refusal", refused=True, code_executed=False)
    core.set(b"core")
    regular = _run(native, project, script, request)
    require(regular.returncode == 0, "regular_core_native_execution_failed")
    records = [document["run"] for document in _documents(regular.stdout) if isinstance(document.get("run"), dict) and isinstance(document["run"].get("result"), dict)]
    require(records, "native_terminal_result_missing")
    record = records[-1]
    result = record["result"]
    require(isinstance(record.get("id"), str) and len(record["id"]) >= 16, "native_private_run_identity_missing")
    scanner.add(record["id"])
    project["native_run_id"] = record["id"]
    expected = (payload_marker + "\n" + source_marker).encode()
    require(allowed.read_bytes() == expected and result["allowed_sha256"] == sha(expected), "native_project_write_failed")
    require(result["home_errno"] in {errno.EPERM, errno.EACCES, errno.EROFS} and result["tmp_errno"] in {errno.EPERM, errno.EACCES, errno.EROFS}, "native_absolute_write_not_denied")
    require(not outside_home.exists() and not outside_tmp.exists(), "native_outside_file_created")
    require(result["core_zero"] and result["core_raise_denied"] and result["dump_raise_denied"] and result["abort_signal"] == 6, "native_core_protection_failed")
    require(not list(view.rglob("core")), "native_core_file_created")
    project["native_allowed_sha256"] = result["allowed_sha256"]
    receipt.add("native_regular_core_execution", actual_cli=True, project_write=True, absolute_home_denied=True, absolute_tmp_denied=True, core_zero=True, core_raise_denied=True, dump_raise_denied=True, abort_signal=6, output_sha256=result["allowed_sha256"])
