"""Hosted proof transport. Private request/CLI bodies remain in RAM, never evidence files."""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import time
from urllib.parse import quote, urlsplit

DOMAIN = "cozy.project-containment.proof.v1"


# Closed fixture case codes only; never serialize a free-form exception message.
_FAILURE_CODES = frozenset({
    'a_lock_after_b_saved', 'a_lock_unexpected_refusal', 'actual_private_browser_flags',
    'actual_private_browser_redirects', 'b_edit_not_persisted', 'born_encrypted',
    'browser_birth_reused', 'browser_durable_storage', 'browser_edit_not_visible',
    'browser_ownership_distinct', 'busy_edit_resume', 'busy_lock_lost_b_draft',
    'chrome_missing', 'cli_exit', 'closed_health_sink',
    'cold_page_ready', 'composed_candidate_not_ready', 'core_control_not_authorized',
    'core_pattern_invalid', 'crash_page_ready', 'crashed_browser_tree_not_retired',
    'crashed_profile_not_retired', 'csrf_no_store_refusal', 'development_config_changed',
    'development_config_shape', 'development_hosted_only', 'development_native_bearer_bridge_present',
    'development_owned_paths', 'development_pnpm_missing', 'development_private_path_forwarded',
    'development_probe_replaced', 'development_process_replaced', 'development_server_exited',
    'development_server_readiness', 'encrypted_create_identity', 'exact_unlocked_project_root',
    'fixture_failure', 'fixture_requires_stopped_calcifer', 'foreign_browser_changed',
    'foreign_browser_start', 'foreign_cleanup_birth', 'foreign_profile_tmpfs',
    'holder_loopback', 'holder_ready', 'hosted_display_missing',
    'hosted_linux_only', 'kill_process_birth', 'last_window_process_tree_alive',
    'last_window_profile_left', 'legacy_public_bootstrap_account', 'legacy_public_bootstrap_cli',
    'legacy_public_bootstrap_database', 'legacy_public_bootstrap_hosted', 'legacy_public_bootstrap_installed_cohort',
    'legacy_public_bootstrap_interface', 'legacy_public_bootstrap_old_holder', 'legacy_public_bootstrap_release_absent',
    'legacy_public_bootstrap_run_list', 'legacy_public_bootstrap_schema_changed', 'legacy_public_bootstrap_source_is_project',
    'legacy_public_bootstrap_static_source', 'legacy_public_bootstrap_unproven_run', 'legacy_public_bootstrap_v7',
    'legacy_public_bootstrap_version', 'lock_closed_admission_before_browser_drain', 'lock_discarded_pending_b_edit',
    'lock_not_started', 'native_absolute_write_not_denied', 'native_allowed_preexists',
    'native_auth_required', 'native_capability', 'native_core_file_created',
    'native_core_protection_failed', 'native_file_not_retained_after_restart', 'native_history_not_retained_after_restart',
    'native_http_status', 'native_outside_file_created', 'native_private_run_identity_missing',
    'native_project_write_failed', 'native_terminal_result_missing', 'old_holder_authority_survived',
    'outside_baseline_replaced', 'outside_project_canary', 'owned_browser_tree_not_retired',
    'owned_hmr_csp_header', 'owned_hmr_initial_module', 'owned_hmr_nonce_missing',
    'owned_hmr_reloaded_document', 'owned_hmr_update', 'owned_host_core_control_required',
    'owned_host_core_control_unavailable', 'owned_no_store_200', 'owned_no_store_206',
    'owned_no_store_conditional', 'owned_page_ready', 'owned_profile_volatile',
    'piped_core_not_refused', 'piped_core_refusal_not_specific', 'private_browser_argv_canary',
    'proof_action', 'proof_fd5_in_use', 'proof_frame',
    'proof_handshake', 'proof_process_birth', 'real_browser_edit',
    'receipt_schema', 'regular_core_native_execution_failed', 'reopened_page_ready',
    'response_object', 'restart_holder_ready', 'scan_decoder_unavailable',
    'scan_markers_missing', 'shared_owned_browser', 'whole_browser_retirement',
})

class Failure(Exception):
    """Only fixed case codes may leave the fixture; never include response/error bodies."""
    def __init__(self, code: str):
        self.code = code if code in _FAILURE_CODES else "fixture_failure"
        super().__init__(self.code)


def require(ok: object, code: str) -> None:
    if not ok:
        raise Failure(code)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def birth(pid: int) -> dict[str, object]:
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return {"pid": pid, "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(), "start_ticks": int(fields[19])}


def alive(expected: dict[str, object]) -> bool:
    try:
        fields = Path(f"/proc/{expected['pid']}/stat").read_text().rsplit(") ", 1)[1].split()
        return fields[0] not in {"Z", "X"} and birth(int(expected["pid"])) == expected
    except FileNotFoundError:
        return False


def until(read, wanted, code: str, seconds: float = 60):
    # An explicit qualification deadline, not a production failure heuristic.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = read()
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.1)
            continue
        except Failure as error:
            if str(error) != "proof_action":
                raise
            time.sleep(0.1)
            continue
        if wanted(value):
            return value
        time.sleep(0.1)
    raise Failure(code)


class Receipt:
    def __init__(self, path: Path, candidate: str):
        self.path, self.value = path, {"candidate_sha256": candidate, "cases": {}, "complete": False}

    def add(self, case: str, **facts: object) -> None:
        def safe(value: object) -> bool:
            if value is None or isinstance(value, (bool, int, float)):
                return True
            if isinstance(value, str):
                return re.fullmatch(r"[0-9a-f]{64}", value) is not None
            if isinstance(value, list):
                return all(safe(item) for item in value)
            if isinstance(value, dict):
                return all(re.fullmatch(r"[a-z][a-z0-9_]*", key) and safe(item) for key, item in value.items())
            return False
        require(re.fullmatch(r"[a-z][a-z0-9_]*", case) and safe(facts), "receipt_schema")
        self.value["cases"][case] = facts
        self.save()

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(json.dumps(self.value, sort_keys=True, indent=2) + "\n")


@dataclass
class Reply:
    status: int
    headers: dict[str, str]
    body: bytes

    def json(self) -> dict:
        value = json.loads(self.body)
        require(isinstance(value, dict), "response_object")
        return value


class Native:
    def __init__(self, cli: Path):
        self.cli = cli

    def command(self, *words: str, cwd: Path | None = None, stdin: bytes | None = None, success: bool = True) -> bytes:
        # Do not use e2e's argv.log/raw .ndjson wrappers for private qualification data.
        deadline = time.monotonic() + 90
        while True:
            completed = subprocess.run([str(self.cli), "--json", *words], input=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd)
            if success and completed.returncode and (words[:2] == ("vault", "lock") or words[:1] == ("open",)):
                try:
                    busy = json.loads(completed.stdout).get("error", {}).get("code") == "project_busy"
                except (ValueError, AttributeError):
                    busy = False
                if busy and time.monotonic() < deadline:
                    time.sleep(0.1)
                    continue
            require((completed.returncode == 0) == success, "cli_exit")
            return completed.stdout

    def document(self, *words: str, cwd: Path | None = None, stdin: bytes | None = None) -> dict:
        return json.loads(self.command(*words, cwd=cwd, stdin=stdin))

    def holder(self) -> dict:
        return json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_bytes())

    def raw(self, method: str, path: str, body: bytes | dict | None = None, *, auth: bool = True, extra: dict[str, str] | None = None, preflight: bool = True) -> Reply:
        holder = self.holder()
        endpoint = urlsplit("http://" + holder["address"])
        require(endpoint.hostname in {"127.0.0.1", "::1", "localhost"}, "holder_loopback")
        if preflight and path != "/v1/project-storage":
            cap = self.raw("GET", "/v1/project-storage", preflight=False).json()
            expected = sha(json.dumps(birth(holder["pid"]), separators=(",", ":")).encode())
            require(cap.get("version") == 3 and cap.get("principal") == sha(holder["token"].encode()) and cap.get("daemon_birth") == expected and self.holder() == holder, "native_capability")
        headers = {"cozy-project-storage": "3", "Cozy-Client": "cli"}
        if auth:
            headers["Authorization"] = "Bearer " + holder["token"]
        if isinstance(body, dict):
            body = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        elif body is not None:
            headers["Content-Type"] = "application/octet-stream"
        if method not in {"GET", "HEAD"}:
            headers["Idempotency-Key"] = "proof-" + os.urandom(16).hex()
        headers.update(extra or {})
        connection = HTTPConnection(endpoint.hostname, endpoint.port, timeout=60)
        try:
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            return Reply(response.status, {key.lower(): value for key, value in response.getheaders()}, response.read())
        finally:
            connection.close()

    def call(self, method: str, path: str, body: bytes | dict | None = None, **options) -> dict:
        response = self.raw(method, path, body, **options)
        require(200 <= response.status < 300, "native_http_status")
        return response.json() if response.body else {}


def project_route(reference: str, suffix: str = "") -> str:
    return "/v1/projects/" + quote(reference, safe="") + suffix


class Proof:
    """The one inherited socket; peer authority is validated again inside real Calcifer."""
    def __init__(self, cli: Path):
        parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        parent.settimeout(90)
        parent.sendall((DOMAIN + "\n").encode())
        descriptor = child.fileno()
        # Never replace an unrelated parent descriptor. Reserve the one child-only slot before
        # spawn; pass_fds explicitly preserves it, with no arbitrary pre_exec Python callback.
        copied = descriptor != 5
        if copied:
            require(not self._open(5), "proof_fd5_in_use")
            os.dup2(descriptor, 5, inheritable=True)
        try:
            self.process = subprocess.Popen([str(cli), "calcifer"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, pass_fds=(5,))
        finally:
            if copied:
                os.close(5)
        child.close()
        self.socket, self.reader = parent, parent.makefile("rb")
        self.birth = birth(self.process.pid)
        self._write({"domain": DOMAIN, "parent": birth(os.getpid()), "calcifer": self.birth})
        hello = self._read()
        require(hello.get("domain") == DOMAIN and hello.get("ready") is True and hello.get("calcifer") == self.birth, "proof_handshake")

    @staticmethod
    def _open(fd: int) -> bool:
        try:
            os.fstat(fd)
            return True
        except OSError:
            return False

    def _write(self, value: dict) -> None:
        self.socket.sendall(json.dumps(value, separators=(",", ":")).encode() + b"\n")

    def _read(self) -> dict:
        line = self.reader.readline((2 << 20) + 1)
        require(0 < len(line) <= 2 << 20, "proof_frame")
        return json.loads(line)

    def ask(self, request: str, **fields) -> dict:
        require(alive(self.birth), "proof_process_birth")
        self._write({"request": request, **fields})
        response = self._read()
        require(response.get("ok") is True, "proof_action")
        return response["observed"]

    def exercise(self, project: str, action: str, **payload) -> dict:
        return self.ask("exercise_fixed", project=project, action={"action": action, **payload})

    def close_channel(self) -> None:
        self.reader.close()
        self.socket.close()


def kill_exact(expected: dict[str, object]) -> None:
    if not alive(expected):
        return
    descriptor = os.pidfd_open(int(expected["pid"]))
    try:
        require(birth(int(expected["pid"])) == expected, "kill_process_birth")
        signal.pidfd_send_signal(descriptor, signal.SIGKILL)
    finally:
        os.close(descriptor)
