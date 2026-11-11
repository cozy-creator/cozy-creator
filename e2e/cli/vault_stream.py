"""Real mounted conversion: receipt, reconnect, no duplicate admission, and missing media.

Runs only inside the cloud project-vault fixture. Synthetic credentials never enter
arguments to the service, logs, progress, or the durable operation receipt.
"""
import hashlib
from collections.abc import Iterable, Iterator
from http.client import HTTPResponse
import json
import os
from pathlib import Path
import sqlite3
import sys
import time
import urllib.parse
import urllib.request
import uuid
from typing import TypedDict, cast


class VaultStatus(TypedDict):
    encrypted: bool
    state: str
    view: str


class Changed(TypedDict, total=False):
    vault: VaultStatus
    skipped_files: list[str]
    recovery_secret: str


class VaultEvent(TypedDict, total=False):
    type: str
    error: dict[str, str]
    changed: Changed


def events(response: Iterable[bytes]) -> Iterator[VaultEvent]:
    data: list[str] = []
    for raw in response:
        line = raw.decode().rstrip("\r\n")
        if line.startswith("data:"):
            data.append(line[5:].lstrip())
        elif not line and data:
            yield cast(VaultEvent, json.loads("\n".join(data)))
            data = []


def main() -> None:
    project, credential, recovery = map(Path, sys.argv[1:])
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    operation = uuid.uuid4().hex
    url = f"http://{holder['address']}/v1/projects/{urllib.parse.quote(str(project.resolve()), safe='')}/vault/operations/{operation}"
    body = json.dumps({"action": "encrypt", "passphrase": credential.read_text().strip(),
                       "remove_plaintext": True, "idle_timeout_s": 0, "max_duration_s": 86400}).encode()
    headers = {"Authorization": f"Bearer {holder['token']}", "Content-Type": "application/json", "Accept": "text/event-stream"}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def connect() -> HTTPResponse:
        return cast(HTTPResponse, opener.open(urllib.request.Request(url, data=body, headers=headers, method="PUT"), timeout=120))

    missing = project / "files/missing-legacy-image.jpg"
    if os.name != "nt":
        missing.symlink_to(project.parent / "absent-original.jpg")
    began = time.monotonic()
    with connect() as first:
        assert first.headers.get_content_type() == "text/event-stream"
        assert "no-store" in first.headers.get("Cache-Control", "")
        assert next(events(first))["type"] == "accepted"
        assert time.monotonic() - began < 5, "receipt waited for the storage operation"
    # The first connection disappears immediately after receipt. The same ID must
    # observe the existing mutation, even if it has already completed.
    kinds: list[str] = []
    changed: Changed | None = None
    with connect() as second:
        for event in events(second):
            kinds.append(event["type"])
            if event["type"] == "failed":
                raise AssertionError(event["error"]["code"])
            if event["type"] == "completed":
                changed = event["changed"]
                break
        else:
            raise AssertionError("reattachment ended without the result")
    assert kinds[0] == "accepted"
    assert changed is not None
    assert changed["vault"]["encrypted"] and changed["vault"]["state"] == "unlocked"
    if os.name != "nt":
        assert "files/missing-legacy-image.jpg" in changed["skipped_files"]
        assert not (Path(changed["vault"]["view"]) / "files/missing-legacy-image.jpg").exists()
    secret = changed.pop("recovery_secret")
    descriptor = os.open(recovery, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as target:
        target.write(secret + "\n")
        target.flush()
        os.fsync(target.fileno())
    connection = sqlite3.connect(f"file:{Path.home()}/.cozy/calcifer/state.db?mode=ro", uri=True)
    rows = connection.execute("SELECT id,binding,state FROM vault_operations WHERE id=?", (operation,)).fetchall()
    assert len(rows) == 1 and rows[0][2] == "completed"
    assert secret not in json.dumps(rows)
    assert hashlib.sha256(secret.encode()).hexdigest() != rows[0][1]
    connection.close()
    print(json.dumps({**changed, "stream_proof": {"reattached": True, "admissions": 1, "events": kinds}}))


if __name__ == "__main__":
    main()
