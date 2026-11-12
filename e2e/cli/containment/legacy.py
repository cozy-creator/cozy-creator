#!/usr/bin/env python3
"""Real released-writer upgrade on a fresh owned hosted runner. Never run locally.

All private command output, journal values and process identities stay in RAM. The
only outside receipt contains fixed cases/counts and public executable hashes.
"""
from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import ssl
import sqlite3
import subprocess
import tempfile
import time
from urllib.parse import quote

from common import Failure, Native, Receipt, alive, birth, kill_exact, require, sha, until, project_route
from legacy_seed import publish_old
from scan import Scanner

HOME = Path.home()
COZY = HOME / ".cozy"
CALCIFER = COZY / "calcifer"
MACHINE = COZY / "machine"
INSTALLED = COZY / "bin/cozy"

PHASES = frozenset({
    "startup", "public_bootstrap", "public_reset", "old_subject_create",
    "old_subject_success", "old_subject_failure", "old_subject_retry",
    "old_subject_verify_outputs", "old_subject_verify_logs", "old_subject_verify_numbers",
    "old_subject_snapshot", "old_subject_stop", "candidate_install",
    "candidate_update", "candidate_doctor", "wrong_binding", "native_adoption",
    "calcifer_crash", "calcifer_restart", "rehistory", "encrypt", "lock_scan",
    "move_cold_retry", "complete",
})
ERROR_KINDS = frozenset({"usage", "validation", "not_found", "credential", "forbidden",
                         "conflict", "capacity", "unavailable", "internal"})
# Attached to the receipt in main; command bodies and exception messages never enter it.
CONTEXT: dict = {}
ASSERTIONS = frozenset({"terminal_json_lines", "terminal_completed", "terminal_failed",
    "settled_http_status", "journal_run_present", "output_http_status", "output_digest_present",
    "output_digest_matches", "old_subject_output_marker", "old_subject_log_marker", "old_subject_numbers",
    "worker_owner_pem", "worker_owner_pkcs8", "worker_owner_algorithm", "worker_owner_public_key",
    "calcifer_replacement_birth", "calcifer_replacement_status", "worker_record_shape",
    "worker_projection_owner", "worker_projection_reference", "worker_projection_file",
    "worker_projection_blob", "worker_record_identity", "worker_record_submission",
    "worker_record_unique", "worker_journal_complete", "worker_execution_preserved"})


def check(ok: object, case: str, **counts: int | bool) -> None:
    require(case in ASSERTIONS and all(type(value) in {int, bool} for value in counts.values()), "receipt_schema")
    CONTEXT["assertion"] = {"case": case, "passed": bool(ok), **counts}
    require(ok, "fixture_failure")


def run_shape(run: dict) -> dict:
    items = run.get("output", [])
    return {"has_id": bool(run.get("id")), "has_number": isinstance(run.get("number"), int),
            "completed": run.get("status") == "completed", "failed": run.get("status") == "failed",
            "canceled": run.get("status") == "canceled", "queued": run.get("status") == "queued",
            "in_progress": run.get("status") == "in_progress", "outputs_is_list": isinstance(items, list),
            "outputs": len(items) if isinstance(items, list) else 0,
            "output_urls": sum(isinstance(item, dict) and bool(item.get("url")) for item in items) if isinstance(items, list) else 0}


def checkpoint(receipt: Receipt, phase: str) -> None:
    require(phase in PHASES, "receipt_schema")
    CONTEXT.clear()
    receipt.value["phase"] = phase
    receipt.save()


def command_failure(result: subprocess.CompletedProcess, expected: bool) -> dict:
    failure = {"class": "signal" if result.returncode < 0 else "exit", "returncode": result.returncode,
               "expected": expected, "kind": None, "code_sha256": None, "cause_sha256": None}
    try:
        documents = [json.loads(result.stdout)]
    except (ValueError, TypeError):
        documents = []
        for line in result.stdout.splitlines():
            try:
                documents.append(json.loads(line))
            except (ValueError, TypeError):
                continue
    for document in documents:
        error = document.get("error") if isinstance(document, dict) else None
        if not isinstance(error, dict):
            continue
        kind = error.get("kind")
        failure["kind"] = kind if isinstance(kind, str) and kind in ERROR_KINDS else None
        for key in ("code", "cause"):
            value = error.get(key)
            failure[key + "_sha256"] = sha(value.encode()) if isinstance(value, str) else None
    return failure


def exception_failure(error: BaseException) -> dict:
    # Closed classes only: even a custom exception's class name may contain user data.
    classes = ((Failure, "fixture_assertion"), (subprocess.TimeoutExpired, "timeout"),
               (subprocess.CalledProcessError, "process"), (FileNotFoundError, "not_found"),
               (PermissionError, "permission"), (json.JSONDecodeError, "json"),
               (sqlite3.Error, "sqlite"), (ConnectionError, "connection"),
               (OSError, "io"), (KeyError, "key"), (TypeError, "type"),
               (ValueError, "value"), (KeyboardInterrupt, "interrupted"))
    return {"class": next((name for kind, name in classes if isinstance(error, kind)), "unclassified"),
            "code_sha256": sha(error.code.encode()) if isinstance(error, Failure) else None,
            "errno": error.errno if isinstance(error, OSError) and type(error.errno) is int else None}


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


@contextmanager
def read_db(path: Path):
    require(path.is_file() and not path.is_symlink(), "fixture_failure")
    connection = sqlite3.connect("file:" + quote(str(path), safe="/") + "?mode=ro", uri=True)
    connection.execute("PRAGMA query_only=ON")
    try:
        yield connection
    finally:
        connection.close()


def schema() -> int:
    with read_db(CALCIFER / "state.db") as connection:
        return connection.execute("PRAGMA user_version").fetchone()[0]


def command(cli: Path, *words: str, cwd: Path | None = None, stdin: bytes | None = None, ok: bool = True) -> bytes:
    deadline = time.monotonic() + 90
    while True:
        try:
            result = subprocess.run([str(cli), "--json", *words], cwd=cwd, input=stdin,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1200)
        except (OSError, subprocess.TimeoutExpired) as error:
            CONTEXT["command"] = {**exception_failure(error), "kind": "unavailable",
                                  "cause_sha256": None, "returncode": None, "expected": not ok}
            raise
        if result.returncode:
            CONTEXT["command"] = command_failure(result, expected=not ok)
        else:
            CONTEXT.pop("command", None)
        if ok and result.returncode and words[:1] == ("vault",) and words[-1:] == ("lock",):
            try:
                busy = json.loads(result.stdout).get("error", {}).get("code") == "project_busy"
            except (ValueError, AttributeError):
                busy = False
            if busy and time.monotonic() < deadline:
                time.sleep(0.1)
                continue
        require((result.returncode == 0) == ok, "cli_exit")
        return result.stdout


def exclusive_free(path: Path) -> None:
    # A stale pidfile is never proof of a stopped writer; test its actual lock.
    if not path.exists():
        return
    with path.open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(lock, fcntl.LOCK_UN)


def descendants(root: dict) -> dict[int, dict]:
    found = {int(root["pid"]): root}
    changed = True
    while changed:
        changed = False
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit() or int(entry.name) in found:
                continue
            try:
                fields = (entry / "stat").read_text().rsplit(") ", 1)[1].split()
                parent = int(fields[1])
                if parent in found and alive(found[parent]):
                    found[int(entry.name)] = birth(int(entry.name))
                    changed = True
            except (FileNotFoundError, ProcessLookupError):
                continue
    return found


def stop(native: Native, *, old: bool) -> None:
    holder = native.holder()
    controller = birth(holder["pid"])
    worker_file = MACHINE / "var/lib/cozy/machine/agent.lock.holder"
    worker = json.loads(worker_file.read_bytes()) if worker_file.exists() else None
    tree = descendants(worker) if worker and alive(worker) else {}
    reply = native.raw("POST", "/v1/down", preflight=not old)
    require(reply.status == 200, "fixture_failure")
    until(lambda: alive(controller), lambda value: not value, "fixture_failure", 90)
    for member in reversed(list(tree.values())):
        kill_exact(member)
    for member in tree.values():
        until(lambda member=member: alive(member), lambda value: not value, "fixture_failure", 90)
    exclusive_free(CALCIFER / "calcifer.lock")
    exclusive_free(MACHINE / "var/lib/cozy/machine/agent.lock")


def remove_public(path: Path) -> None:
    """Only used before any private subject exists, on explicitly owned public paths."""
    if not path.exists():
        return
    require(not path.is_symlink(), "fixture_failure")
    if path.is_dir():
        require(not any(child.is_symlink() for child in path.rglob("*")), "fixture_failure")
        shutil.rmtree(path)
    else:
        path.unlink()


def reset_public_bootstrap(native: Native, provenance: dict) -> None:
    # Explicit coordinator approval is limited to this fresh, zero-project public
    # bootstrap. Never call this after generating a canary or creating a project.
    projects = native.call("GET", "/v1/projects?limit=0", preflight=False)
    require(projects.get("projects") == [], "fixture_failure")
    with read_db(CALCIFER / "state.db") as source:
        rows = source.execute("SELECT id,target,kind,state,out_dir FROM runs").fetchall()
        require({row[0] for row in rows} == set(provenance["public_maintenance_run_ids"]), "fixture_failure")
        require(all(row[1] in {"cozy/package_publish", "cozy/package_lock"} and row[2] == "here" and row[3] == "settled" for row in rows), "fixture_failure")
        require(source.execute("SELECT count(*) FROM projects").fetchone()[0] == 0, "fixture_failure")
    stop(native, old=True)
    require(schema() == 7, "fixture_failure")
    # These exact source/cache paths were created only by the verified public
    # publisher in this fresh runner. Account keys and installed tools survive.
    for relative in ("state.db", "state.db-wal", "state.db-shm", "code", "local", "inputs", "after"):
        remove_public(CALCIFER / relative)
    remove_public(COZY / "cache/digests")


def terminal(stdout: bytes, status: str) -> dict:
    values = []
    shape = {"lines": 0, "invalid_json": 0, "objects": 0, "nested_runs": 0, "root_runs": 0,
             "has_id": 0, "completed": 0, "failed": 0, "canceled": 0, "matching": 0,
             "expect_failed": status == "failed"}
    for line in stdout.splitlines():
        shape["lines"] += 1
        try:
            value = json.loads(line)
        except ValueError:
            shape["invalid_json"] += 1
            continue
        if isinstance(value, dict):
            shape["objects"] += 1
            run = value.get("run", value)
            if isinstance(run, dict):
                shape["nested_runs" if "run" in value else "root_runs"] += 1
                shape["has_id"] += bool(run.get("id"))
                for name in ("completed", "failed", "canceled"):
                    shape[name] += run.get("status") == name
                if run.get("status") == status and run.get("id"):
                    values.append(run)
    shape["matching"] = len(values)
    CONTEXT["terminal"] = shape
    check(shape["invalid_json"] == 0, "terminal_json_lines", invalid=shape["invalid_json"])
    check(bool(values), "terminal_failed" if status == "failed" else "terminal_completed", matches=len(values))
    return values[-1]


def settled(native: Native, identifier: str, *, old: bool, reference: str) -> dict:
    path = "/v1/runs/" + quote(identifier, safe="") if old else project_route(reference, "/runs/" + quote(identifier, safe=""))
    def read():
        response = native.raw("GET", path + "?detail=full", preflight=not old)
        check(200 <= response.status < 300, "settled_http_status", status=response.status)
        run = response.json()
        CONTEXT["settled"] = run_shape(run)
        return run
    return until(read, lambda run: run.get("status") in {"completed", "failed", "canceled"}
                 and all(item.get("url") or item.get("export_error") or item.get("stays") or item.get("checkpoint") for item in run.get("output", [])), "fixture_failure", 1200)


def journal(path: Path, identifiers: list[str]) -> dict:
    answer = {}
    with read_db(path) as connection:
        for identifier in identifiers:
            row = connection.execute("SELECT number,idempotency_key,digest,cursor,seq,state_seq,inputs FROM runs WHERE id=?", (identifier,)).fetchone()
            check(row is not None, "journal_run_present", rows=int(row is not None))
            logs = connection.execute("SELECT seq,at_ms,level,text FROM run_logs WHERE run=? ORDER BY seq", (identifier,)).fetchall()
            outputs = connection.execute("SELECT name,idx,rev,digest,length,exported_rev FROM outputs WHERE run=? ORDER BY name,idx", (identifier,)).fetchall()
            answer[identifier] = {"row": row, "logs": logs, "outputs": outputs}
    return answer


def worker_execution(root: Path, number: int, encoded: str) -> dict:
    record = json.loads(encoded)
    check(isinstance(record, dict), "worker_record_shape")
    if "_private_payload" in record:
        # Modern records are a scheduling projection plus an immutable blob in
        # this same project's folder (private_journal.rs), not plaintext SQL.
        # Never follow the reference's supplied root to another filesystem path.
        area = root.resolve(strict=True)
        check(root.name == ".vault-machine" and not root.is_symlink(), "worker_projection_owner")
        project = area.parent
        marker = json.loads((project / ".vault-local-execution.json").read_bytes())
        owner = marker["storage"]
        pointer = record["_private_payload"]
        check(isinstance(pointer, str) and pointer.startswith("cozy-private-blob:1:"), "worker_projection_reference")
        reference = json.loads(pointer.removeprefix("cozy-private-blob:1:"))
        check(isinstance(reference, dict) and set(reference) == {"format", "storage", "context", "file", "length"}, "worker_projection_reference")
        scope = reference["storage"]
        context = "execution:" + str(number)
        check(reference["format"] == 1 and reference["context"] == context
              and isinstance(scope, dict) and scope.get("project_id") == owner.get("project_id")
              and type(scope.get("generation")) is int and scope["generation"] > 0
              and Path(scope.get("view_root", "")) == project, "worker_projection_owner")
        name = reference["file"]
        check(isinstance(name, str) and name.endswith(".blob") and len(name) == 37
              and all(char in "0123456789abcdefABCDEF" for char in name[:-5]), "worker_projection_file")
        directory = area / "journal"
        path = directory / name
        check(directory.is_dir() and not directory.is_symlink() and path.is_file() and not path.is_symlink(), "worker_projection_file")
        data = path.read_bytes()
        check(type(reference["length"]) is int and len(data) == reference["length"], "worker_projection_file")
        blob = json.loads(data)
        check(isinstance(blob, dict) and set(blob) == {"format", "project_id", "generation", "context", "payload"}
              and blob["format"] == 1 and blob["project_id"] == scope["project_id"]
              and blob["generation"] == scope["generation"] and blob["context"] == context,
              "worker_projection_blob")
        decoded = json.loads(base64.b64decode(blob["payload"], validate=True))
        check(isinstance(decoded, dict) and decoded.get("id") == record.get("id")
              and decoded.get("invocation", {}).get("local_private") == scope, "worker_projection_blob")
        record = decoded
    check(record.get("id") == str(number), "worker_record_identity")
    return record


def worker_journal(root: Path, identifiers: list[str]) -> tuple[Path, dict]:
    matches = []
    for path in root.rglob("executions.sqlite3"):
        if path.is_symlink():
            continue
        rows = {}
        with read_db(path) as connection:
            for identifier in identifiers:
                found = connection.execute("SELECT id,record FROM executions WHERE request_id=?", (identifier,)).fetchall()
                check(len(found) <= 1, "worker_record_unique", rows=len(found))
                if found:
                    number, encoded = found[0]
                    record = worker_execution(root, number, encoded)
                    check(record.get("submission", {}).get("request_id") == identifier, "worker_record_submission")
                    rows[identifier] = {"number": number, **{key: record.get(key) for key in
                        ("attempt", "accepted_at_ms", "started_at_ms", "finished_at_ms", "executed_ms", "submission")}}
        if rows:
            matches.append((path, rows))
    check(len(matches) == 1 and set(matches[0][1]) == set(identifiers), "worker_journal_complete",
          journals=len(matches), requested=len(identifiers), found=sum(len(rows) for _, rows in matches))
    return matches[0]


def owner_signer(raw: bytes):
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    CONTEXT["worker_stage"] = "owner_key_decode"
    try:
        private = serialization.load_pem_private_key(raw, password=None)
    except ValueError:
        # Dalek's native write_key emits RFC5958 PKCS#8 v2 with the public key.
        # Some OpenSSL loaders accept only v1. Recognize ONLY this exact Ed25519
        # encoding and let the existing crypto library derive/validate the key.
        lines = raw.strip().splitlines()
        check(len(raw) <= 4096 and len(lines) >= 3 and lines[0] == b"-----BEGIN PRIVATE KEY-----"
              and lines[-1] == b"-----END PRIVATE KEY-----", "worker_owner_pem")
        der = base64.b64decode(b"".join(lines[1:-1]), validate=True)
        check(len(der) == 83 and der[:16] == bytes.fromhex("3051020101300506032b657004220420")
              and der[48:51] == bytes.fromhex("812100"), "worker_owner_pkcs8")
        private = Ed25519PrivateKey.from_private_bytes(der[16:48])
        public = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        check(public == der[51:], "worker_owner_public_key")
    check(isinstance(private, Ed25519PrivateKey), "worker_owner_algorithm")
    return private


def worker_request(project: Path, method: str, suffix: str, body: dict) -> tuple[int, bytes]:
    """Existing TLS-pinned native API, real owner key, no qualification server seam."""
    from cryptography.hazmat.primitives import serialization
    import h2.config
    import h2.connection
    import h2.events
    from urllib.parse import urlsplit

    CONTEXT["worker_stage"] = "registered_endpoint"
    with read_db(CALCIFER / "state.db") as connection:
        endpoint = connection.execute("SELECT kind,worker_id,address,pin,key FROM machines WHERE name='local'").fetchone()
    require(endpoint and endpoint[0] == "local", "fixture_failure")
    _, worker_id, address, pin, key_path = endpoint
    parsed = urlsplit("https://" + address)
    require(parsed.hostname in {"127.0.0.1", "::1", "localhost"} and len(pin) == 32, "fixture_failure")
    marker = json.loads((project / ".vault-local-execution.json").read_bytes())
    private = owner_signer(Path(key_path).read_bytes())
    CONTEXT["worker_stage"] = "capability_sign"
    public = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    b64 = lambda value: base64.urlsafe_b64encode(value).rstrip(b"=").decode()
    payload = json.dumps({"m": worker_id, "a": "machine", "e": int(time.time()) + 300,
                          "k": b64(hashlib.sha256(public).digest()[:16])}, separators=(",", ":")).encode()
    capability = b64(payload) + "." + b64(private.sign(b"cozy-capability/1\0" + payload))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    context.set_alpn_protocols(["h2"])
    encoded = json.dumps(body, separators=(",", ":")).encode()
    path = "/v1/project-execution/" + quote(marker["storage"]["project_id"], safe="") + "/" + suffix
    CONTEXT["worker_stage"] = "worker_connect"
    with socket.create_connection((parsed.hostname, parsed.port), timeout=60) as tcp:
        CONTEXT["worker_stage"] = "worker_tls"
        with context.wrap_socket(tcp, server_hostname=parsed.hostname) as tls:
            # Authentication/private proof is transmitted only after the actual
            # registered native leaf pin is verified on this connection.
            require(hashlib.sha256(tls.getpeercert(binary_form=True)).digest() == pin
                    and tls.selected_alpn_protocol() == "h2", "fixture_failure")
            CONTEXT["worker_stage"] = "worker_h2_request"
            protocol = h2.connection.H2Connection(config=h2.config.H2Configuration(client_side=True, header_encoding="utf-8"))
            protocol.initiate_connection()
            protocol.send_headers(1, [(":method", method), (":scheme", "https"), (":authority", address), (":path", path),
                ("authorization", "Cozy-Cap " + capability), ("content-type", "application/json"),
                ("cozy-local-private-storage", b64(json.dumps(marker["storage"], separators=(",", ":")).encode())),
                ("cozy-local-private-token", marker["token"])])
            protocol.send_data(1, encoded, end_stream=True)
            tls.sendall(protocol.data_to_send())
            CONTEXT["worker_stage"] = "worker_h2_response"
            status, answer = 0, bytearray()
            while True:
                incoming = tls.recv(65536)
                require(incoming, "fixture_failure")
                for event in protocol.receive_data(incoming):
                    if isinstance(event, h2.events.ResponseReceived):
                        status = int(dict(event.headers)[":status"])
                        CONTEXT["worker_http_status"] = status
                    elif isinstance(event, h2.events.DataReceived):
                        answer.extend(event.data)
                        require(len(answer) <= 4 << 20, "fixture_failure")
                        protocol.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                    elif isinstance(event, h2.events.StreamEnded):
                        return status, bytes(answer)
                    elif isinstance(event, (h2.events.StreamReset, h2.events.ConnectionTerminated)):
                        require(False, "fixture_failure")
                pending = protocol.data_to_send()
                if pending:
                    tls.sendall(pending)


def wrong_binding_stays_pending(native: Native, reference: str, project: Path, original_worker: Path, identifiers: list[str], receipt: Receipt) -> list[dict]:
    # Real source contention prevents the automatic bridge from retiring legacy
    # rows on this first read. No fabricated row or test-only barrier is used.
    CONTEXT["worker_stage"] = "source_contention"
    blocker = sqlite3.connect(original_worker, timeout=0)
    try:
        blocker.execute("BEGIN IMMEDIATE")
        CONTEXT["worker_stage"] = "controller_history_read"
        native.call("GET", project_route(reference, "/runs?limit=0"))
    finally:
        blocker.rollback()
        blocker.close()
    with read_db(project / "project.db") as connection:
        require(connection.execute("SELECT count(*) FROM runs WHERE custody='legacy_pending'").fetchone()[0] >= len(identifiers), "fixture_failure")
    status, body = worker_request(project, "POST", "migration-bindings", {"request_ids": identifiers})
    require(status == 200, "fixture_failure")
    lookup = json.loads(body)["bindings"]
    require({value["binding"]["request_id"] for value in lookup} == set(identifiers), "fixture_failure")
    invalid = [dict(value["binding"]) for value in lookup]
    actual = invalid[0]["invocation_spec_digest"]
    invalid[0]["invocation_spec_digest"] = actual[:-1] + ("0" if actual[-1] != "0" else "1")
    before = worker_journal(MACHINE, identifiers)[1]
    status, _ = worker_request(project, "POST", "migrate", {"legacy_requests": invalid})
    require(status == 409, "fixture_failure")
    require(worker_journal(MACHINE, identifiers)[1] == before, "fixture_failure")
    with read_db(CALCIFER / "state.db") as connection:
        for identifier in identifiers:
            require(connection.execute("SELECT count(*) FROM legacy_project_runs WHERE id=?", (identifier,)).fetchone()[0] == 1, "fixture_failure")
    receipt.add("legacy_wrong_binding", passed=True, source_retained=True, canonical_pending=True, status=status)
    return [value["binding"] for value in lookup]


def crash_before_controller_receipt(native: Native, project: Path, bindings: list[dict], receipt: Receipt) -> None:
    # An actual worker completes its durable copy/retirement, then the exact Caf
    # dies before any controller history read can keep that receipt. The next
    # holder must recover real bindings from the inside receipt, not resubmit.
    checkpoint(receipt, "native_adoption")
    status, _ = worker_request(project, "POST", "migrate", {"legacy_requests": bindings})
    require(status == 204, "fixture_failure")
    status, body = worker_request(project, "GET", "migrate", {})
    covered = json.loads(body).get("bindings", []) if status == 200 else []
    require(status == 200 and all(binding in covered for binding in bindings), "fixture_failure")
    with read_db(project / "project.db") as connection:
        require(connection.execute("SELECT count(*) FROM runs WHERE custody='legacy_pending'").fetchone()[0] >= len(bindings), "fixture_failure")
    checkpoint(receipt, "calcifer_crash")
    CONTEXT["crash_stage"] = "holder_read"
    holder = native.holder()
    CONTEXT["crash_stage"] = "birth_read"
    exact = birth(holder["pid"])
    CONTEXT["crash_stage"] = "signal_exact_birth"
    kill_exact(exact)
    CONTEXT["crash_stage"] = "wait_exact_birth_ended"
    until(lambda: alive(exact), lambda value: not value, "fixture_failure", 90)
    # Restart=on-failure may already have installed a fresh holder. An unheld
    # lock is not required: prove the killed Birth ended and its replacement is
    # a different live daemon with positive protocol-3 authority instead.
    checkpoint(receipt, "calcifer_restart")
    CONTEXT["crash_stage"] = "doctor_recovery"
    command(INSTALLED, "doctor")
    CONTEXT["crash_stage"] = "replacement_holder"
    replacement = birth(native.holder()["pid"])
    check(replacement != exact and alive(replacement), "calcifer_replacement_birth",
          different=replacement != exact, live=alive(replacement))
    CONTEXT["crash_stage"] = "replacement_capability"
    # Native.raw positively binds version, token and actual daemon Birth on this
    # global status read; it does not read/acknowledge the project receipt.
    response = native.raw("GET", "/v1/status")
    check(response.status == 200, "calcifer_replacement_status", status=response.status)


def output_bytes(native: Native, run: dict, *, old: bool) -> list[bytes]:
    bodies = []
    for item in run.get("output", []):
        if item.get("url") and not item.get("stays"):
            response = native.raw("GET", item["url"], preflight=not old)
            check(response.status == 200, "output_http_status", status=response.status)
            # v0.24 intentionally omits OutputItem.sha256 for a failed run's
            # incomplete output. Its finished revision still advertises the
            # canonical digest in this authenticated HTTP response.
            expected = item.get("sha256") or response.headers.get("x-cozy-digest")
            check(isinstance(expected, str), "output_digest_present", advertised=bool(item.get("sha256")), header=bool(response.headers.get("x-cozy-digest")), incomplete=item.get("status") == "incomplete")
            check("sha256:" + sha(response.body) == expected, "output_digest_matches", advertised=bool(item.get("sha256")))
            bodies.append(response.body)
    return bodies


def assert_history(native: Native, reference: str, project: Path, baseline: dict, results: dict) -> None:
    listed = native.call("GET", project_route(reference, "/runs?limit=0"))
    require(set(baseline).issubset({run["id"] for run in listed["runs"]}), "fixture_failure")
    for identifier, expected in results.items():
        current = settled(native, identifier, old=False, reference=reference)
        require(current["number"] == expected["number"] and current["status"] == expected["status"] and current.get("result") == expected.get("result"), "fixture_failure")
        require(output_bytes(native, current, old=False) == expected["bodies"], "fixture_failure")
    require(journal(project / "project.db", list(baseline)) == baseline, "fixture_failure")


def complete_migration(native: Native, reference: str, project: Path) -> None:
    def read():
        native.call("GET", project_route(reference, "/runs?limit=0"))
        with read_db(project / "project.db") as connection:
            return connection.execute("SELECT count(*) FROM runs WHERE custody='legacy_pending'").fetchone()[0]
    until(read, lambda count: count == 0, "fixture_failure", 180)
    with read_db(CALCIFER / "state.db") as connection:
        for table in ("legacy_project_runs", "legacy_project_outputs", "legacy_project_run_logs"):
            require(connection.execute("SELECT count(*) FROM " + table).fetchone()[0] == 0, "fixture_failure")


def outside_scan(markers: list[bytes], allowed: list[Path], receipt: Receipt) -> dict:
    scanner = Scanner()
    for marker in markers:
        scanner.add(marker)
    roots = [HOME, Path("/tmp"), Path("/var/tmp"), Path("/dev/shm"), Path(os.environ["RUNNER_TEMP"])]
    counts = scanner.scan(roots, allowed, receipt.path)
    require(not counts.get("matches"), "outside_project_canary")
    require(not any(counts.get(key) for key in ("unreadable", "sqlite_unreadable", "archive_unreadable", "archive_oversize", "leveldb_unreadable", "images_unreadable")), "scan_decoder_unavailable")
    # A matched path hash would itself identify private source custody. Keep
    # numeric boundaries only in this fixture's outside evidence receipt.
    return {key: value for key, value in counts.items() if isinstance(value, int)}


def qualify(args, receipt: Receipt) -> None:
    checkpoint(receipt, "startup")
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_OS") == "Linux"
            and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "hosted_linux_only")
    ready = json.loads(args.ready_file.read_bytes())
    require(ready.get("ready") is True and ready.get("sha256") == digest(args.candidate), "composed_candidate_not_ready")
    require(digest(INSTALLED) == digest(args.old), "fixture_failure")
    require(subprocess.check_output([str(args.old), "--version"]).strip() == b"cozy 0.24.0", "fixture_failure")
    work = Path(tempfile.mkdtemp(prefix="cozy-legacy-public-", dir=os.environ["RUNNER_TEMP"]))
    # The skew runner passes the installed path. Preserve only this public
    # executable before replacing the default worker launcher later.
    old_copy = work / "cozy-0.24.0"
    shutil.copy2(args.old, old_copy)
    old_copy.chmod(0o755)
    args.old = old_copy
    old = Native(args.old)
    checkpoint(receipt, "public_bootstrap")
    provenance = publish_old(args.old, work)
    checkpoint(receipt, "public_reset")
    reset_public_bootstrap(old, provenance)
    receipt.add("old_public_bootstrap", passed=True, old_binary_sha256=digest(args.old), source_sha256=provenance["source_sha256"])

    # No reset is legal after this point. Every following byte is real old
    # application data and must be migrated or explicitly remain pending.
    checkpoint(receipt, "old_subject_create")
    project = Path(tempfile.mkdtemp(prefix="cozy-legacy-project-", dir=os.environ["RUNNER_TEMP"]))
    opened = json.loads(command(args.old, "new", str(project)))
    reference = opened["id"]
    require(schema() == 7 and (project / "project.db").is_file(), "fixture_failure")
    marker = ("legacy-private-" + os.urandom(24).hex()).encode()
    source = project / "source.txt"
    source.write_bytes(marker + b"-input")
    source_bytes = source.read_bytes()
    source_sha = sha(source_bytes)
    results = {}
    for fail in (False, True):
        checkpoint(receipt, "old_subject_failure" if fail else "old_subject_success")
        request = project / ("failure.json" if fail else "success.json")
        request.write_text(json.dumps({"text": marker.decode() + ("-failed" if fail else "-success"), "fail": fail}))
        # The released CLI rejects --gpus 0; the public callable's actual
        # accelerator=False contract selects CPU without that flag.
        raw = command(args.old, "run", provenance["target"], "--input", str(request), "--file", "source=" + str(source), "--on", "local", cwd=project, ok=not fail)
        run = terminal(raw, "failed" if fail else "completed")
        run = settled(old, run["id"], old=True, reference=reference)
        run["bodies"] = output_bytes(old, run, old=True)
        results[run["id"]] = run
    checkpoint(receipt, "old_subject_verify_outputs")
    marker_bodies = sum(marker in body for run in results.values() for body in run["bodies"])
    check(marker_bodies > 0, "old_subject_output_marker", runs=len(results), bodies=sum(len(run["bodies"]) for run in results.values()), marker_bodies=marker_bodies)
    baseline = journal(CALCIFER / "state.db", list(results))
    checkpoint(receipt, "old_subject_verify_logs")
    marker_logs = sum(marker in log[3].encode() for value in baseline.values() for log in value["logs"])
    logged_runs = sum(any(marker in log[3].encode() for log in value["logs"]) for value in baseline.values())
    check(logged_runs == len(baseline), "old_subject_log_marker", runs=len(baseline), logged_runs=logged_runs,
          logs=sum(len(value["logs"]) for value in baseline.values()), marker_logs=marker_logs)
    checkpoint(receipt, "old_subject_verify_numbers")
    check(all(value["row"][0] > 0 for value in baseline.values()), "old_subject_numbers", runs=len(baseline))
    # Keep a known real old HTTP request/key too, so a byte-identical retry after
    # move can establish old digest compatibility without reconstructing a CLI ask.
    checkpoint(receipt, "old_subject_retry")
    retry_key = "legacy-retry-" + os.urandom(16).hex()
    retry_ask = {"target": provenance["target"], "on": "local", "project": str(project),
                 "input": {"source": "sha256:" + source_sha, "text": marker.decode() + "-retry"}}
    response = old.call("POST", "/v1/runs", retry_ask, preflight=False, extra={"Idempotency-Key": retry_key})
    require(not response.get("preparing"), "fixture_failure")
    run = settled(old, response["run"]["id"], old=True, reference=reference)
    run["bodies"] = output_bytes(old, run, old=True)
    results[run["id"]] = run
    checkpoint(receipt, "old_subject_snapshot")
    baseline = journal(CALCIFER / "state.db", list(results))
    original_worker, worker_baseline = worker_journal(MACHINE, list(results))
    require(schema() == 7, "fixture_failure")
    receipt.add("released_v7_subject", passed=True, runs=len(results), logs=sum(len(value["logs"]) for value in baseline.values()), outputs=sum(len(value["outputs"]) for value in baseline.values()))
    checkpoint(receipt, "old_subject_stop")
    stop(old, old=True)

    # The installed default is also the worker launcher. Replace it only after
    # both exact old process cohorts and native writer locks are gone.
    checkpoint(receipt, "candidate_install")
    temporary_binary = INSTALLED.with_name("cozy.legacy-candidate")
    shutil.copy2(args.candidate, temporary_binary)
    temporary_binary.chmod(0o755)
    temporary_binary.replace(INSTALLED)
    native = Native(INSTALLED)
    updates = ["machine", "update", "local", "--tensord", str(args.candidate)]
    if args.runtime:
        updates += ["--runtime", args.runtime]
    if args.tensorfs:
        updates += ["--tensorfs", args.tensorfs]
    checkpoint(receipt, "candidate_update")
    command(INSTALLED, *updates, "-y")
    checkpoint(receipt, "candidate_doctor")
    command(INSTALLED, "doctor")
    require(schema() == 8, "fixture_failure")
    checkpoint(receipt, "wrong_binding")
    bindings = wrong_binding_stays_pending(native, reference, project, original_worker, list(results), receipt)
    crash_before_controller_receipt(native, project, bindings, receipt)
    checkpoint(receipt, "rehistory")
    complete_migration(native, reference, project)
    receipt.add("legacy_worker_receipt_crash_recovery", passed=True, no_resubmission=True, runs=len(results))
    check(worker_journal(project / ".vault-machine", list(results))[1] == worker_baseline, "worker_execution_preserved", runs=len(results))
    assert_history(native, reference, project, baseline, results)
    # Input closure is required after settlement too; this is not just an API
    # view over a retained outside journal or a currently warm worker.
    require(any(path.is_file() and path.read_bytes() == source_bytes for path in (project / ".vault-inputs").rglob("*") if not path.is_symlink()), "fixture_failure")
    replay = native.call("POST", project_route(reference, "/runs/submit"), retry_ask, extra={"Idempotency-Key": retry_key})
    require(replay.get("replay") is True and replay["run"]["id"] == run["id"], "fixture_failure")
    require(journal(project / "project.db", list(results)) == baseline, "fixture_failure")
    require(worker_journal(project / ".vault-machine", list(results))[1] == worker_baseline, "fixture_failure")
    # An actual old client cannot bypass the schema/HTTP protocol fence.
    rejected = subprocess.run([str(args.old), "--json", "run", "watch", str(run["number"]), "--once"], cwd=project,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    require(rejected.returncode != 0, "fixture_failure")
    require(journal(project / "project.db", list(results)) == baseline, "fixture_failure")
    receipt.add("canonical_legacy_migration", passed=True, runs=len(results), same_keys_and_numbers=True, exact_log_and_output_rows=True, old_retry_replayed=True, old_client_refused=True)

    checkpoint(receipt, "encrypt")
    secret = os.urandom(32).hex().encode() + b"\n"
    changed = json.loads(command(INSTALLED, "vault", "--directory", str(project), "encrypt", "--passphrase-file", "-", stdin=secret))
    view = Path(changed["vault"]["view"])
    identity = native.call("GET", project_route(str(project), "/address"))
    require(identity["id"] == reference, "fixture_failure")
    assert_history(native, reference, view, baseline, results)
    checkpoint(receipt, "lock_scan")
    command(INSTALLED, "vault", "--directory", str(project), "lock")
    markers = [marker, source_sha.encode(), *(item[3].removeprefix("sha256:").encode() for value in baseline.values() for item in value["outputs"]),
               *(identifier.encode() for identifier in baseline),
               *(value["row"][1].encode() for value in baseline.values()),
               *(value["row"][2].encode() for value in baseline.values()),
               *(value["submission"]["invocation_digest"].encode() for value in worker_baseline.values())]
    # A locked encrypted folder is not a plaintext allowance. Scan its backing,
    # keybag and any leftover conversion files along with every outside root.
    scan = outside_scan(markers, [], receipt)
    receipt.add("legacy_encrypted_retirement", passed=True, **scan)
    checkpoint(receipt, "move_cold_retry")
    stop(native, old=False)
    moved = project.with_name(project.name + "-moved")
    project.rename(moved)
    # Cold holder and real unlock after moving the opaque encrypted folder.
    command(INSTALLED, "vault", "--directory", str(moved), "unlock", "--passphrase-file", "-", stdin=secret)
    status = native.call("GET", project_route(str(moved), "/vault"))
    view = Path(status["view"])
    identity = native.call("GET", project_route(str(moved), "/address"))
    require(identity["id"] == reference, "fixture_failure")
    assert_history(native, reference, view, baseline, results)
    replay = native.call("POST", project_route(reference, "/runs/submit"), retry_ask, extra={"Idempotency-Key": retry_key})
    require(replay.get("replay") is True and replay["run"]["id"] == run["id"], "fixture_failure")
    require(worker_journal(view / ".vault-machine", list(results))[1] == worker_baseline, "fixture_failure")
    command(INSTALLED, "vault", "--directory", str(moved), "lock")
    stop(native, old=False)
    receipt.add("legacy_move_cold_retry", passed=True, runs=len(results), **outside_scan(markers, [], receipt))
    checkpoint(receipt, "complete")
    receipt.value["complete"] = True
    receipt.save()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--old", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--ready-file", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--runtime", default="")
    parser.add_argument("--tensorfs", default="")
    args = parser.parse_args()
    args.old, args.candidate = args.old.resolve(), args.candidate.resolve()
    receipt = Receipt(args.out / "legacy-receipt.json", digest(args.candidate))
    receipt.value["context"] = CONTEXT
    try:
        qualify(args, receipt)
        return 0
    except BaseException as error:
        CONTEXT["exception"] = exception_failure(error)
        receipt.value["complete"] = False
        receipt.save()
        # Exception text, subprocess output and paths can carry private content.
        print("legacy_containment_incomplete", file=__import__("sys").stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
