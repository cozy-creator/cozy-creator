"""Hosted, fixed-public CPU call to an owned second Unix user's real default-home worker.

No alternate Cozy home, rental, raw artifact, or private-canary exception. Native command
output and remote journal inspection stay in RAM; the receipt contains hashes/counts only.
"""
from __future__ import annotations

import contextlib
import base64
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import sqlite3
import subprocess
from typing import TypedDict, cast
from urllib.parse import quote, urlsplit

from common import Failure, Native, Project, Ready, Receipt, RunDetail, project_run_ids, require, sha, until, project_route
from native_execution import _documents, _observation

TEXT = "Cozy public remote CPU fixture v1"
SOURCE = b"Cozy public remote input v1"
PROGRAM = '''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import msgspec
from cozy_runtime.author import App, Outputs, Telemetry
from cozy_runtime.media import File
app = App()
class Request(msgspec.Struct, forbid_unknown_fields=True):
    source: File
    text: str
class Answer(msgspec.Struct):
    file: File
    text: str
@app.entrypoint(accelerator=False)
def echo(payload: Request, out: Outputs, tel: Telemetry) -> Answer:
    # Runtime2ba9 author/_services.py1974 and _signature.py516/539 inject Telemetry.
    # Its log at _services.py1652 is an instance method, not a static service call.
    content = payload.source.read_bytes() + b"\\n" + payload.text.encode()
    tel.log(payload.text, level="info")
    target = out.temporary_file(".txt")
    target.write_bytes(content)
    return Answer(out.save_file(target, media_type="text/plain"), payload.text)
'''

CONFIGURE = '''import json,os
from pathlib import Path
p = Path.home()/".cozy"
p.mkdir(mode=0o700, exist_ok=True)
if p.is_symlink() or not p.is_dir(): raise ValueError()
(p/"config.toml").write_text('[machine]\\nlisten_host = "127.0.0.1"\\n')
# Closed facts of the session environment this user's cozy inherits through sudo.
runtime=os.environ.get("XDG_RUNTIME_DIR")
try: own=bool(runtime) and os.stat(runtime).st_uid == os.getuid()
except OSError: own=False
print(json.dumps({"runtime_dir_set":bool(runtime),"runtime_dir_own":own,"session_bus_set":bool(os.environ.get("DBUS_SESSION_BUS_ADDRESS"))}))
'''
JOURNAL = '''import json,sqlite3,sys
from pathlib import Path
ask=json.load(sys.stdin)
found=[]
for path in (Path.home()/".cozy/machine").rglob("executions.sqlite3"):
    if path.is_symlink(): continue
    db=sqlite3.connect("file:"+str(path)+"?mode=ro",uri=True)
    db.execute("PRAGMA query_only=ON")
    found.extend(json.loads(row[0]) for row in db.execute("SELECT record FROM executions WHERE request_id=?",(ask["id"],)))
    db.close()
print(json.dumps(found))
'''
CLOSE = '''import json,os,signal,time,tomllib
from pathlib import Path
home=Path.home()/".cozy"
config=tomllib.loads((home/"config.toml").read_text())
# Match MachineConfig.root + Home.machine, rather than guessing from a primary user's
# explicit machine.root override. The fixture user has its own default home/config.
root=Path(config.get("machine",{}).get("root",str(home/"machine")))
if not root.is_absolute() or not root.is_relative_to(home): raise ValueError()
boot=Path("/proc/sys/kernel/random/boot_id").read_text().strip()
def birth(pid):
    path=Path(f"/proc/{pid}")
    if path.stat().st_uid != os.getuid(): return None
    fields=(path/"stat").read_text().rsplit(") ",1)[1].split()
    return (pid,int(fields[19]))
def live(item):
    try:
        fields=Path(f"/proc/{item[0]}/stat").read_text().rsplit(") ",1)[1].split()
        return fields[0] not in {"Z","X"} and birth(item[0]) == item
    except (FileNotFoundError,ProcessLookupError): return False
owned={}
seen={"caf":False,"worker":False}
for role,path in (("caf",home/"calcifer/calcifer.lock.holder"),("worker",root/"var/lib/cozy/machine/agent.lock.holder")):
    if path.exists():
        try:
            record=json.loads(path.read_bytes())
            item=birth(record["pid"])
            # A record naming another user's or an older process is nothing of ours to retire.
            if item is None or role == "worker" and (record.get("boot_id") != boot or record.get("start_ticks") != item[1]): continue
            if not live(item): continue
            owned[item[0]]=item
            seen[role]=True
        except (FileNotFoundError,ProcessLookupError): pass
changed=True
while changed:
    changed=False
    for path in Path("/proc").iterdir():
        if not path.name.isdigit() or int(path.name) in owned: continue
        try:
            fields=(path/"stat").read_text().rsplit(") ",1)[1].split()
            if int(fields[1]) in owned and live(owned[int(fields[1])]):
                item=birth(int(path.name))
                if item: owned[item[0]]=item; changed=True
        except (FileNotFoundError,ProcessLookupError,PermissionError): pass
for item in reversed(list(owned.values())):
    if not live(item): continue
    fd=os.pidfd_open(item[0])
    try:
        if live(item): signal.pidfd_send_signal(fd,signal.SIGKILL)
    finally: os.close(fd)
deadline=time.monotonic()+60
while any(map(live,owned.values())) and time.monotonic()<deadline: time.sleep(.1)
print(json.dumps({"gone":not any(map(live,owned.values())),"processes":len(owned),"worker_observed":seen["worker"],"caf_observed":seen["caf"]}))
'''


class Worker:
    def __init__(self, native: Native, receipt: Receipt):
        self.native, self.receipt = native, receipt
        self.user = "cozy-remote-" + secrets.token_hex(5)
        self.name = self.user
        self.binary = Path("/opt") / self.user / "cozy"
        self.created = False
        self.current_stage = "start"
        self.first_failed = False
        self.cleanup_failed = False

    def stage(self, stage: str) -> None:
        self.current_stage = stage
        if not self.first_failed:
            self.receipt.add("remote_cpu_stage", **{"stage_" + stage: True})

    def failure(self, error: BaseException, *, completed: subprocess.CompletedProcess[bytes] | None = None, cleanup: bool = False) -> None:
        if not cleanup and self.first_failed:
            return
        facts: dict[str, object] = {"stage_" + ("retire" if cleanup else self.current_stage): True}
        classes = {"Failure": "failure", "ValueError": "value_error", "KeyError": "key_error", "TypeError": "type_error",
                   "FileNotFoundError": "file_not_found", "FileExistsError": "file_exists", "PermissionError": "permission_error",
                   "OSError": "os_error", "JSONDecodeError": "json_decode", "OperationalError": "sqlite_operational"}
        facts["exception_" + classes.get(type(error).__name__, "other")] = True
        if isinstance(error, OSError) and isinstance(error.errno, int):
            facts["errno"] = error.errno
        if completed is not None:
            facts.update(Native.failure_facts(completed, ()))
            for name, literal in (("file_exists", b"FileExistsError"), ("file_not_found", b"FileNotFoundError"),
                                  ("permission", b"PermissionError"), ("value", b"ValueError"), ("key", b"KeyError")):
                facts["stderr_exception_" + name] = literal in completed.stderr
            errno = re.search(rb"\[Errno ([0-9]{1,4})\]", completed.stderr)
            if errno:
                facts["stderr_errno"] = int(errno[1])
        self.receipt.add("remote_cpu_cleanup_failure" if cleanup else "remote_cpu_failure", **facts)
        if cleanup:
            self.cleanup_failed = True
        else:
            self.first_failed = True

    def call(self, stage: str, words: list[str], *, body: bytes | None = None, user: bool = False) -> bytes:
        cleanup = stage == "retire"
        if cleanup:
            self.receipt.add("remote_cpu_cleanup_stage", stage_retire=True)
        else:
            self.stage(stage)
        command = ["sudo", "-n"]
        if user:
            # The primary runner may have explicit layout overrides. They must not move
            # this fresh Unix user's worker/store into another user's Cozy root.
            command += ["-u", self.user, "-H", "env", "-u", "COZY_MACHINE_ROOT", "-u", "COZY_TENSORFS_ROOT",
                        "-u", "COZY_TENSORFS_CACHE", "-u", "TENSORFS_HOME", "COZY_HUB_URL=" + os.environ["COZY_HUB_URL"]]
            if os.environ.get("SSL_CERT_FILE"):
                command += ["SSL_CERT_FILE=" + os.environ["SSL_CERT_FILE"]]
        try:
            completed = subprocess.run(command + words, input=body, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd="/")
        except BaseException as error:
            self.failure(error, cleanup=cleanup)
            raise
        if completed.returncode:
            failed = Failure("fixture_failure")
            self.failure(failed, completed=completed, cleanup=cleanup)
            raise failed
        return completed.stdout

    def start(self, ready: Ready) -> None:
        try:
            pwd.getpwnam(self.user)
            require(False, "fixture_failure")
        except KeyError:
            pass
        self.call("user", ["useradd", "-m", self.user]); self.created = True
        self.call("binary", ["install", "-D", "-m", "755", str(self.native.cli), str(self.binary)])
        environment = json.loads(self.call("config", ["python3", "-c", CONFIGURE], user=True))
        self.receipt.add("remote_cpu_user_environment", **{key: environment.get(key) is True for key in ("runtime_dir_set", "runtime_dir_own", "session_bus_set")})
        self.stage("primary_identity")
        primary = self.native.document("machine", "show", "local")
        remote = json.loads(self.call("identity", [str(self.binary), "--json", "machine", "show", "local"], user=True))
        key, fingerprint = primary.get("controller_key"), remote.get("fingerprint")
        if not (isinstance(key, str) and isinstance(fingerprint, str)):
            raise Failure("fixture_failure")
        authorized = json.loads(self.call("authorize", [str(self.binary), "--json", "machine", "authorize", key], user=True))
        # `reach` is where other computers add it, none on loopback (tensord serve.rs `reach`); this
        # loopback stand-in is added at its pinned `address`, `host:port` as `reach` would be.
        reach = authorized.get("machine", {}).get("address")
        require(isinstance(reach, str) and urlsplit("//" + reach).hostname in {"127.0.0.1", "::1", "localhost"}, "fixture_failure")
        self.stage("add")
        self.native.document("machine", "add", self.name, reach, "--fingerprint", fingerprint)
        updates: list[str] = []
        for flag, version in (("runtime", ready.get("runtime")), ("tensorfs", ready.get("tensorfs"))):
            if version: updates += ["--" + flag, version]
        if updates:
            self.stage("update")
            self.native.command("machine", "update", self.name, *updates, "-y")
        self.receipt.add("remote_cpu_worker", default_user_home=True, loopback=True, fingerprint_pinned=True,
                         public_controller_authorized=True, binary_sha256=sha(self.native.cli.read_bytes()))

    def close(self, *, primary_failed: bool = False) -> None:
        if not self.created: return
        try:
            observed = json.loads(self.call("retire", ["python3", "-c", CLOSE], user=True))
            count = observed.get("processes", 0)
            self.receipt.add("remote_cpu_retirement", gone=observed.get("gone") is True, processes=count,
                             worker_observed=observed.get("worker_observed") is True, caf_observed=observed.get("caf_observed") is True)
            require(observed.get("gone") is True, "fixture_failure")
            # After a failed start there may be nothing to retire; a completed run's worker must have been.
            require(primary_failed or isinstance(count, int) and count > 0 and observed.get("worker_observed") is True, "fixture_failure")
        except BaseException as error:
            if not self.cleanup_failed:
                self.failure(error, cleanup=True)
            self.receipt.add("remote_cpu_retirement", gone=False)
            raise


# An encrypted project's run text in project.db is a pointer to a record in its own view
# (calcifer project/vault.rs `seal_payload`): read as the product's `open_payload` reads it.
SEALED = "cozy:vault:journal:2:"


class History(TypedDict):
    row: tuple[object, ...]
    logs: list[tuple[object, ...]]
    outputs: list[tuple[object, ...]]


def opened(view: Path, value: object) -> str:
    if not isinstance(value, str):
        raise Failure("remote_history_text")
    if not value.startswith(SEALED):
        return value
    reference = json.loads(value.removeprefix(SEALED))
    require(isinstance(reference, dict) and re.fullmatch(r"[0-9a-f]{32}", str(reference.get("object", ""))), "remote_history_journal")
    at = view / ".vault-journal" / reference["object"]
    require(at.is_file() and not at.is_symlink(), "remote_history_journal")
    record = json.loads(at.read_bytes())
    require(isinstance(record, dict) and record.get("project_id") == reference.get("project_id")
            and record.get("context") == reference.get("context"), "remote_history_journal")
    return base64.b64decode(record["data"], validate=True).decode()


def history(view: Path, identifier: str) -> History:
    path = view / "project.db"
    require(path.is_file() and not path.is_symlink(), "fixture_failure")
    # sqlite3's context manager never closes; a lingering handle would keep the project view busy.
    with contextlib.closing(sqlite3.connect("file:" + quote(str(path), safe="/") + "?mode=ro", uri=True)) as connection:
        connection.execute("PRAGMA query_only=ON")
        row = connection.execute("SELECT number,inputs,idempotency_key,digest FROM runs WHERE id=?", (identifier,)).fetchone()
        logs = connection.execute("SELECT seq,level,text FROM run_logs WHERE run=? ORDER BY seq", (identifier,)).fetchall()
        outputs = connection.execute("SELECT name,idx,digest,length,exported_rev FROM outputs WHERE run=? ORDER BY name,idx", (identifier,)).fetchall()
    require(row is not None and outputs and any(TEXT in opened(view, log[2]) for log in logs), "fixture_failure")
    return {"row": row, "logs": logs, "outputs": outputs}


def qualify(native: Native, project: Project, receipt: Receipt, ready: Ready) -> None:
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "hosted_linux_only")
    worker = Worker(native, receipt)
    primary_failed = False
    try:
        worker.start(ready)
        worker.stage("source")
        folder = project["view"] / "public-remote-cpu"
        folder.mkdir(mode=0o700)
        script, source, request = folder / "echo.py", folder / "source.txt", folder / "input.json"
        script.write_text(PROGRAM); source.write_bytes(SOURCE); request.write_text(json.dumps({"text": TEXT}))
        worker.stage("run")
        # `--input` takes a FILE (cli run/mod.rs `read_input`); `-` is not stdin.
        words = ("run", str(script) + "/echo", "--input", str(request), "--file", "source=" + str(source), "--on", worker.name)
        completed = subprocess.run([str(native.cli), "--json", *words], stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=project["folder"])
        documents = _documents(completed.stdout)
        runs = [document.get("run", document) for document in documents]
        # A failed run ends in run documents, not an error envelope: its closed classes, as the managed run's.
        receipt.add("remote_cpu_run_observation", **_observation(completed, documents, runs, False))
        if completed.returncode:
            native.failure = native.failure or Native.failure_facts(completed, words)
            raise Failure("cli_exit")
        terminal = [run for run in runs if isinstance(run, dict) and run.get("status") == "completed" and run.get("result", {}).get("text") == TEXT]
        require(terminal, "fixture_failure")
        run = terminal[-1]
        require(run.get("machine") == worker.name and run.get("result", {}).get("text") == TEXT, "fixture_failure")
        identifier = run["id"]
        worker.stage("detail")
        detail_path = project_route(project["id"], "/runs/" + quote(identifier, safe="") + "?detail=full")
        detail = until(lambda: cast(RunDetail, native.call("GET", detail_path)),  # cast: the run's JSON
                       lambda value: value.get("status") == "completed" and any(item.get("url") for item in value.get("output", [])), "fixture_failure", 1200)
        expected = SOURCE + b"\n" + TEXT.encode()
        items = [item for item in detail.get("output", []) if item.get("url") and not item.get("stays")]
        require(len(items) == 1, "fixture_failure")
        item = items[0]
        output = project_route(project["id"], "/runs/" + quote(identifier, safe="") + "/outputs/" + quote(item["name"], safe=""))
        if item.get("index"): output += "?index=" + str(item["index"])
        worker.stage("output")
        response = native.raw("GET", output)
        require(response.status == 200 and response.body == expected, "fixture_failure")
        worker.stage("local_history")
        before = history(project["view"], identifier)
        require(sha(SOURCE) in opened(project["view"], before["row"][1]), "fixture_failure")
        require(any(path.is_file() and not path.is_symlink() and path.read_bytes() == SOURCE
                    for path in (project["view"] / ".vault-inputs").rglob("*")), "fixture_failure")
        proof = json.loads((project["view"] / ".vault-local-execution.json").read_bytes())
        remote = json.loads(worker.call("journal", ["python3", "-c", JOURNAL], user=True, body=json.dumps({"id": identifier}).encode()))
        require(len(remote) == 1 and remote[0]["invocation"].get("local_private") is None, "fixture_failure")
        recorded = json.dumps(remote, sort_keys=True)
        require(str(project["view"]) not in recorded and proof["token"] not in recorded and "cozy-local-private" not in recorded, "fixture_failure")
        worker.close(); worker.created = False
        worker.stage("retained_history")
        retained = native.call("GET", project_route(project["id"], "/runs/" + quote(identifier, safe="") + "?detail=full"))
        listed, _ = project_run_ids(lambda path: native.call("GET", path), project["id"])
        require(retained.get("status") == "completed" and identifier in listed, "fixture_failure")
        require(history(project["view"], identifier) == before and native.raw("GET", output).body == expected, "fixture_failure")
        receipt.add("remote_cpu_execution", explicit_remote=True, actual_cli=True, input_in_project=True, output_matches=True,
                    local_history_retained=True, remote_invocation_unscoped=True, local_view_absent=True, local_proof_absent=True,
                    input_sha256=sha(SOURCE), output_sha256=sha(expected), runs=1, outputs=len(items), logs=len(before["logs"]))
    except BaseException as error:
        primary_failed = True
        if not worker.cleanup_failed:
            worker.failure(error)
        raise
    finally:
        if worker.created and not worker.cleanup_failed:
            try:
                worker.close(primary_failed=primary_failed)
            except BaseException:
                # Cleanup has its own closed receipt; preserve the original work exception.
                if not primary_failed:
                    raise
