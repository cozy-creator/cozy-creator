"""Hosted proof transport. Private request/CLI bodies remain in RAM, never evidence files."""
from __future__ import annotations

from collections.abc import Callable
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
from typing import NotRequired, TypedDict, TypeVar, Unpack, cast
from urllib.parse import quote, urlsplit

DOMAIN = "cozy.project-containment.proof.v1"
T = TypeVar("T")  # not PEP 695 syntax: legacy.sh's `uv run` may pick a Python before 3.12


class Birth(TypedDict):
    """A process's exact identity: its pid within this boot, and its start time."""
    pid: int
    boot_id: str
    start_ticks: int


class Holder(TypedDict):
    """Calcifer's lock holder record (the fields these fixtures read)."""
    pid: int
    address: str
    token: str


class Ready(TypedDict, total=False):
    """The coordinator's ready file."""
    ready: bool
    sha256: str
    development_web: bool
    native_execution: bool
    remote_cpu: bool
    runtime: str
    tensorfs: str


class Project(TypedDict):
    """A fixture project, in RAM only."""
    folder: Path
    view: Path
    id: str
    credential: str
    marker: str
    edit: str
    asset: str
    path: str
    digest: str
    text_file: str
    native_run_id: NotRequired[str]
    native_allowed_sha256: NotRequired[str]


class Observed(TypedDict, total=False):
    """The proof channel's observation of an action (the fields these fixtures read)."""
    present: bool
    pages: int
    processes: list[Birth]
    birth: Birth
    profile: str
    retirement_incomplete: bool
    page_ready: bool
    cards: int
    typed: bool
    visible: bool
    authenticated: bool
    status: int
    no_store: bool
    range: bool
    body_sha256: str
    development: bool
    value: int
    csp_nonce: str
    refresh_nonce: str
    style_nonce: str
    document_sha256: str


class RunRow(TypedDict):
    id: str
    number: int


class VaultStatus(TypedDict, total=False):
    encrypted: bool
    state: str
    view: str


class OutputItem(TypedDict, total=False):
    name: str
    index: int
    url: str
    stays: bool
    sha256: str
    status: str
    export_error: object
    checkpoint: object


class RunDetail(TypedDict, total=False):
    """A run's JSON (`?detail=full`), the fields these fixtures read."""
    id: str
    number: int
    status: str
    output: list[OutputItem]
    result: dict[str, object]
    bodies: list[bytes]


class RunPage(TypedDict, total=False):
    runs: list[RunRow]
    unreadable: list[object]
    older: bool


class ReceiptValue(TypedDict, total=False):
    candidate_sha256: str
    cases: dict[str, dict[str, object]]
    complete: bool
    phase: str
    context: dict[str, object]
    failed_case: str
    error_class: str
    failed_check: str
    diagnostic_rejected: bool
    cleanup_incomplete: bool
    development_restore_failed: bool


class RawOptions(TypedDict, total=False):
    auth: bool
    extra: dict[str, str] | None
    preflight: bool


# Closed fixture case codes only; never serialize a free-form exception message.
_FAILURE_CODES = frozenset({
    "managed_execution_failed", "managed_path_outside_project", "managed_environment_in_project", "managed_cache_contents", "managed_core_limit",
    'a_lock_after_b_saved', 'a_lock_unexpected_refusal', 'actual_private_browser_flags',
    'actual_private_browser_redirects', 'b_edit_not_persisted', 'born_encrypted',
    'browser_birth_reused', 'browser_durable_storage', 'browser_edit_not_visible',
    'browser_ownership_distinct', 'busy_edit_resume', 'busy_lock_lost_b_draft',
    'caf_crash_page_ready', 'caf_crash_browser_tree_not_retired', 'caf_crash_profile_not_retired',
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
    'last_window_profile_left', 'lock_closed_admission_before_browser_drain', 'lock_discarded_pending_b_edit',
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
    'scan_markers_missing', 'scan_process', 'shared_owned_browser', 'whole_browser_retirement',
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


def birth(pid: int) -> Birth:
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return {"pid": pid, "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(), "start_ticks": int(fields[19])}


def alive(expected: Birth) -> bool:
    try:
        fields = Path(f"/proc/{expected['pid']}/stat").read_text().rsplit(") ", 1)[1].split()
        return fields[0] not in {"Z", "X"} and birth(int(expected["pid"])) == expected
    except FileNotFoundError:
        return False


def until(read: Callable[[], T], wanted: Callable[[T], object], code: str, seconds: float = 60) -> T:
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
        self.path = path
        self.value: ReceiptValue = {"candidate_sha256": candidate, "cases": {}, "complete": False}

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

    def json(self) -> dict[str, object]:
        value = json.loads(self.body)
        require(isinstance(value, dict), "response_object")
        return cast(dict[str, object], value)  # cast: the response's JSON object, checked above


class Native:
    def __init__(self, cli: Path):
        self.cli = cli
        self.failure: dict[str, object] | None = None

    @staticmethod
    def failure_facts(completed: subprocess.CompletedProcess[bytes], words: tuple[str, ...]) -> dict[str, object]:
        # Closed classifications or hashes only. Never return messages, metadata, argv,
        # response bodies, stderr text, project references or credentials to a receipt.
        facts: dict[str, object] = {
            "exit_code": completed.returncode,
            "stdout_sha256": sha(completed.stdout),
            "stderr_sha256": sha(completed.stderr),
            "action_open": words[:1] == ("open",),
            "action_vault": words[:1] == ("vault",),
        }
        error = None
        try:
            document = json.loads(completed.stdout)
            if isinstance(document, dict) and isinstance(document.get("error"), dict):
                error = document["error"]
        except (ValueError, TypeError):
            pass
        if error is None:
            for line in reversed(completed.stdout.splitlines()):
                try:
                    document = json.loads(line)
                except (ValueError, TypeError):
                    continue
                if isinstance(document, dict) and isinstance(document.get("error"), dict):
                    error = document["error"]
                    break
        if error is None:
            # A plain command's closed `error[code]` and `cause:` lines (stderr), as its JSON envelope says them.
            code = re.search(rb"^error\[([a-z0-9_]+)\]", completed.stderr, re.M)
            cause = re.search(rb"^cause: ([a-z0-9_]+)$", completed.stderr, re.M)
            if code:
                error = {"code": code.group(1).decode(), **({"cause": cause.group(1).decode()} if cause else {})}
        facts["error_envelope"] = error is not None
        if error is None:
            return facts
        allowed = {
            "code": {"project_busy", "project_locked", "project_storage_update_required", "project_storage_migration_pending",
                     "private_browser_unavailable", "private_browser_profile", "private_browser_recovery_pending",
                     "private_browser_retirement_incomplete", "browser_open_needs_bearer", "project_not_found",
                     "project_required", "project_storage_unsupported", "calcifer_unreachable", "unauthenticated"},
            "cause": {"browser_executable", "browser_home", "browser_profile_setup", "browser_capture", "browser_seal", "browser_pipe", "browser_spawn",
                      "browser_birth", "browser_ownership", "browser_context", "browser_target", "browser_attach",
                      "browser_inject", "browser_navigate", "browser_activate"},
            "kind": {"usage", "validation", "not_found", "credential", "forbidden", "conflict", "capacity", "unavailable", "internal"},
        }
        for field, values in allowed.items():
            value = error.get(field)
            if isinstance(value, str):
                if value in values:
                    facts[field + "_" + value] = True
                else:
                    facts[field + "_sha256"] = sha(value.encode())
        metadata = error.get("metadata")
        if isinstance(metadata, dict):
            protocol = metadata.get("browser_protocol_code")
            if isinstance(protocol, int) and not isinstance(protocol, bool) and protocol in {-32700, -32600, -32601, -32602, -32603, -32000}:
                facts["browser_protocol_code"] = protocol
            for field in ("browser_startup_unreadable", "browser_startup_bounded", "browser_startup_no_sandbox", "browser_startup_no_display",
                          "browser_startup_permission_denied", "browser_startup_invalid_descriptor", "browser_startup_no_space", "browser_startup_socket_path_too_long",
                          "browser_startup_namespace", "browser_startup_crashpad", "browser_startup_fatal", "browser_startup_devtools_pipe",
                          "browser_startup_zygote", "browser_startup_logging", "browser_startup_profile", "browser_startup_allocator",
                          "browser_startup_ozone_display", "browser_startup_dbus", "browser_startup_user_namespace", "browser_startup_icu",
                          "browser_startup_ptrace", "browser_startup_shared_memory", "browser_startup_invalid_flag",
                          "browser_process_exited", "browser_exit_before_retirement", "browser_startup_retired"):
                if isinstance(metadata.get(field), bool):
                    facts[field] = metadata[field]
            for field in ("browser_startup_bytes", "browser_exit_code", "browser_exit_signal"):
                value = metadata.get(field)
                if value is None or isinstance(value, int) and not isinstance(value, bool):
                    facts[field] = value
            digest = metadata.get("browser_startup_sha256")
            if isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest):
                facts["browser_startup_sha256"] = digest
        return facts

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
            if (completed.returncode == 0) != success:
                if self.failure is None:
                    self.failure = self.failure_facts(completed, words)
                raise Failure("cli_exit")
            return completed.stdout

    def open(self, cwd: Path) -> None:
        # The project in Cozy's own private browser: since cl-980 `--json open` only prints a link and starts none.
        deadline = time.monotonic() + 90
        while True:
            completed = subprocess.run([str(self.cli), "open"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd)
            if completed.returncode and re.search(rb"^error\[project_busy\]", completed.stderr, re.M) and time.monotonic() < deadline:
                time.sleep(0.1)
                continue
            if completed.returncode:
                if self.failure is None:
                    self.failure = self.failure_facts(completed, ("open",))
                raise Failure("cli_exit")
            return

    def document(self, *words: str, cwd: Path | None = None, stdin: bytes | None = None) -> dict[str, object]:
        return cast(dict[str, object], json.loads(self.command(*words, cwd=cwd, stdin=stdin)))  # cast: the CLI's --json document

    def holder(self) -> Holder:
        return cast(Holder, json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_bytes()))  # cast: the holder record's JSON

    def raw(self, method: str, path: str, body: bytes | dict[str, object] | None = None, *, auth: bool = True, extra: dict[str, str] | None = None, preflight: bool = True) -> Reply:
        holder = self.holder()
        endpoint = urlsplit("http://" + holder["address"])
        if endpoint.hostname not in {"127.0.0.1", "::1", "localhost"}:
            raise Failure("holder_loopback")
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

    def call(self, method: str, path: str, body: bytes | dict[str, object] | None = None, **options: Unpack[RawOptions]) -> dict[str, object]:
        response = self.raw(method, path, body, **options)
        require(200 <= response.status < 300, "native_http_status")
        return response.json() if response.body else {}


def project_route(reference: str, suffix: str = "") -> str:
    return "/v1/projects/" + quote(reference, safe="") + suffix


def project_run_ids(get: Callable[[str], dict[str, object]], reference: str) -> tuple[set[str], int]:
    """Every listed run id and how many were unreadable. The API clamps `limit` to 1..1000,
    newest first (so `limit=0` is one run); page by `before` as `cozy run ls` does."""
    found: set[str] = set()
    unreadable, before = 0, ""
    while True:
        page = cast(RunPage, get(project_route(reference, "/runs?limit=1000" + before)))  # cast: the run list's JSON
        found.update(run.get("id") for run in page["runs"])
        unreadable += len(page.get("unreadable") or [])
        if not page.get("older") or not page["runs"]:
            return found, unreadable
        before = "&before=" + str(page["runs"][-1]["number"])


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

    def _write(self, value: dict[str, object]) -> None:
        self.socket.sendall(json.dumps(value, separators=(",", ":")).encode() + b"\n")

    def _read(self) -> dict[str, object]:
        line = self.reader.readline((2 << 20) + 1)
        require(0 < len(line) <= 2 << 20, "proof_frame")
        return cast(dict[str, object], json.loads(line))  # cast: the proof channel's JSON frame

    def ask(self, request: str, **fields: object) -> Observed:
        require(alive(self.birth), "proof_process_birth")
        self._write({"request": request, **fields})
        response = self._read()
        require(response.get("ok") is True, "proof_action")
        return cast(Observed, response["observed"])  # cast: the proof channel's JSON observation

    def exercise(self, project: str, action: str, **payload: object) -> Observed:
        return self.ask("exercise_fixed", project=project, action={"action": action, **payload})

    def close_channel(self) -> None:
        self.reader.close()
        self.socket.close()


def kill_exact(expected: Birth) -> None:
    if not alive(expected):
        return
    descriptor = os.pidfd_open(int(expected["pid"]))
    try:
        require(birth(int(expected["pid"])) == expected, "kill_process_birth")
        signal.pidfd_send_signal(descriptor, signal.SIGKILL)
    finally:
        os.close(descriptor)
