"""Hosted, fixed-public CPU call to an owned second Unix user's real default-home worker.

No alternate Cozy home, rental, raw artifact, or private-canary exception. Native command
output and remote journal inspection stay in RAM; the receipt contains hashes/counts only.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import pwd
import secrets
import sqlite3
import subprocess
from urllib.parse import quote, urlsplit

from common import Native, Receipt, require, sha, until, project_route
from native_execution import _documents

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
    content = payload.source.read_bytes() + b"\\n" + payload.text.encode()
    tel.log(payload.text, level="info")
    target = out.temporary_file(".txt")
    target.write_bytes(content)
    return Answer(out.save_file(target, media_type="text/plain"), payload.text)
'''

CONFIGURE = '''from pathlib import Path
p = Path.home()/".cozy"
p.mkdir(mode=0o700)
(p/"config.toml").write_text('[machine]\\nlisten_host = "127.0.0.1"\\n')
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
CLOSE = '''import json,os,signal,time
from pathlib import Path
def birth(pid):
    path=Path(f"/proc/{pid}")
    if path.stat().st_uid != os.getuid(): raise ValueError()
    fields=(path/"stat").read_text().rsplit(") ",1)[1].split()
    return (pid,int(fields[19]))
def live(item):
    try:
        fields=Path(f"/proc/{item[0]}/stat").read_text().rsplit(") ",1)[1].split()
        return fields[0] not in {"Z","X"} and birth(item[0]) == item
    except (FileNotFoundError,ProcessLookupError): return False
owned={}
for relative in ("calcifer/calcifer.lock.holder","machine/var/lib/cozy/machine/agent.lock.holder"):
    path=Path.home()/".cozy"/relative
    if path.exists():
        try:
            item=birth(json.loads(path.read_bytes())["pid"])
            owned[item[0]]=item
        except (FileNotFoundError,ProcessLookupError): pass
changed=True
while changed:
    changed=False
    for path in Path("/proc").iterdir():
        if not path.name.isdigit() or int(path.name) in owned: continue
        try:
            fields=(path/"stat").read_text().rsplit(") ",1)[1].split()
            if int(fields[1]) in owned and live(owned[int(fields[1])]):
                item=birth(int(path.name)); owned[item[0]]=item; changed=True
        except (FileNotFoundError,ProcessLookupError,PermissionError): pass
for item in reversed(list(owned.values())):
    if not live(item): continue
    fd=os.pidfd_open(item[0])
    try:
        if live(item): signal.pidfd_send_signal(fd,signal.SIGKILL)
    finally: os.close(fd)
deadline=time.monotonic()+60
while any(map(live,owned.values())) and time.monotonic()<deadline: time.sleep(.1)
print(json.dumps({"gone":not any(map(live,owned.values())),"processes":len(owned)}))
'''


class Worker:
    def __init__(self, native: Native, receipt: Receipt):
        self.native, self.receipt = native, receipt
        self.user = "cozy-remote-" + secrets.token_hex(5)
        self.name = self.user
        self.binary = Path("/opt") / self.user / "cozy"
        self.created = False

    def call(self, stage: str, words: list[str], *, body: bytes | None = None, user: bool = False) -> bytes:
        self.receipt.add("remote_cpu_stage", **{"stage_" + stage: True})
        command = ["sudo", "-n"]
        if user:
            command += ["-u", self.user, "-H", "env", "COZY_HUB_URL=" + os.environ["COZY_HUB_URL"]]
            if os.environ.get("SSL_CERT_FILE"):
                command += ["SSL_CERT_FILE=" + os.environ["SSL_CERT_FILE"]]
        completed = subprocess.run(command + words, input=body, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd="/")
        if completed.returncode:
            self.receipt.add("remote_cpu_failure", **{"stage_" + stage: True}, exit_code=completed.returncode,
                             stdout_sha256=sha(completed.stdout), stderr_sha256=sha(completed.stderr))
        require(completed.returncode == 0, "fixture_failure")
        return completed.stdout

    def start(self, ready: dict) -> None:
        try:
            pwd.getpwnam(self.user)
            require(False, "fixture_failure")
        except KeyError:
            pass
        self.call("user", ["useradd", "-m", self.user]); self.created = True
        self.call("binary", ["install", "-D", "-m", "755", str(self.native.cli), str(self.binary)])
        self.call("config", ["python3", "-c", CONFIGURE], user=True)
        primary = self.native.document("machine", "show", "local")
        remote = json.loads(self.call("identity", [str(self.binary), "--json", "machine", "show", "local"], user=True))
        key, fingerprint = primary.get("controller_key"), remote.get("fingerprint")
        require(isinstance(key, str) and isinstance(fingerprint, str), "fixture_failure")
        authorized = json.loads(self.call("authorize", [str(self.binary), "--json", "machine", "authorize", key], user=True))
        reach = authorized.get("machine", {}).get("reach")
        require(isinstance(reach, str) and urlsplit(reach).hostname in {"127.0.0.1", "::1", "localhost"}, "fixture_failure")
        self.native.document("machine", "add", self.name, reach, "--fingerprint", fingerprint)
        updates = []
        for flag in ("runtime", "tensorfs"):
            if ready.get(flag): updates += ["--" + flag, ready[flag]]
        if updates:
            self.native.command("machine", "update", self.name, *updates, "-y")
        self.receipt.add("remote_cpu_worker", default_user_home=True, loopback=True, fingerprint_pinned=True,
                         public_controller_authorized=True, binary_sha256=sha(self.native.cli.read_bytes()))

    def close(self) -> None:
        if not self.created: return
        try:
            observed = json.loads(self.call("retire", ["python3", "-c", CLOSE], user=True))
            self.receipt.add("remote_cpu_retirement", gone=observed.get("gone") is True, processes=observed.get("processes", 0))
            require(observed.get("gone") is True, "fixture_failure")
        except BaseException:
            self.receipt.add("remote_cpu_retirement", gone=False)
            raise


def history(view: Path, identifier: str) -> dict:
    path = view / "project.db"
    require(path.is_file() and not path.is_symlink(), "fixture_failure")
    with sqlite3.connect("file:" + quote(str(path), safe="/") + "?mode=ro", uri=True) as connection:
        connection.execute("PRAGMA query_only=ON")
        row = connection.execute("SELECT number,inputs,idempotency_key,digest FROM runs WHERE id=?", (identifier,)).fetchone()
        logs = connection.execute("SELECT seq,level,text FROM run_logs WHERE run=? ORDER BY seq", (identifier,)).fetchall()
        outputs = connection.execute("SELECT name,idx,digest,length,exported_rev FROM outputs WHERE run=? ORDER BY name,idx", (identifier,)).fetchall()
    require(row is not None and outputs and any(TEXT in log[2] for log in logs), "fixture_failure")
    return {"row": row, "logs": logs, "outputs": outputs}


def qualify(native: Native, project: dict, receipt: Receipt, ready: dict) -> None:
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "hosted_linux_only")
    worker = Worker(native, receipt)
    try:
        worker.start(ready)
        folder = project["view"] / "public-remote-cpu"
        folder.mkdir(mode=0o700)
        script, source = folder / "echo.py", folder / "source.txt"
        script.write_text(PROGRAM); source.write_bytes(SOURCE)
        body = native.command("run", str(script) + "/echo", "--input", "-", "--file", "source=" + str(source),
                              "--on", worker.name, cwd=project["folder"], stdin=json.dumps({"text": TEXT}).encode())
        runs = [document.get("run", document) for document in _documents(body)]
        terminal = [run for run in runs if isinstance(run, dict) and run.get("status") == "completed" and run.get("result", {}).get("text") == TEXT]
        require(terminal, "fixture_failure")
        run = terminal[-1]
        require(run.get("machine") == worker.name and run.get("result", {}).get("text") == TEXT, "fixture_failure")
        identifier = run["id"]
        detail_path = project_route(project["id"], "/runs/" + quote(identifier, safe="") + "?detail=full")
        detail = until(lambda: native.call("GET", detail_path),
                       lambda value: value.get("status") == "completed" and any(item.get("url") for item in value.get("output", [])), "fixture_failure", 1200)
        expected = SOURCE + b"\n" + TEXT.encode()
        items = [item for item in detail.get("output", []) if item.get("url") and not item.get("stays")]
        require(len(items) == 1, "fixture_failure")
        item = items[0]
        output = project_route(project["id"], "/runs/" + quote(identifier, safe="") + "/outputs/" + quote(item["name"], safe=""))
        if item.get("index"): output += "?index=" + str(item["index"])
        response = native.raw("GET", output)
        require(response.status == 200 and response.body == expected, "fixture_failure")
        before = history(project["view"], identifier)
        require(sha(SOURCE) in before["row"][1], "fixture_failure")
        require(any(path.is_file() and not path.is_symlink() and path.read_bytes() == SOURCE
                    for path in (project["view"] / ".vault-inputs").rglob("*")), "fixture_failure")
        proof = json.loads((project["view"] / ".vault-local-execution.json").read_bytes())
        remote = json.loads(worker.call("journal", ["python3", "-c", JOURNAL], user=True, body=json.dumps({"id": identifier}).encode()))
        require(len(remote) == 1 and remote[0]["invocation"].get("local_private") is None, "fixture_failure")
        recorded = json.dumps(remote, sort_keys=True)
        require(str(project["view"]) not in recorded and proof["token"] not in recorded and "cozy-local-private" not in recorded, "fixture_failure")
        worker.close(); worker.created = False
        retained = native.call("GET", project_route(project["id"], "/runs/" + quote(identifier, safe="") + "?detail=full"))
        listed = native.call("GET", project_route(project["id"], "/runs?limit=0"))
        require(retained.get("status") == "completed" and any(row.get("id") == identifier for row in listed["runs"]), "fixture_failure")
        require(history(project["view"], identifier) == before and native.raw("GET", output).body == expected, "fixture_failure")
        receipt.add("remote_cpu_execution", explicit_remote=True, actual_cli=True, input_in_project=True, output_matches=True,
                    local_history_retained=True, remote_invocation_unscoped=True, local_view_absent=True, local_proof_absent=True,
                    input_sha256=sha(SOURCE), output_sha256=sha(expected), runs=1, outputs=len(items), logs=len(before["logs"]))
    finally:
        worker.close()
