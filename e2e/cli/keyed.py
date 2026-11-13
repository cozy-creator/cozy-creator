"""A keyed write cut off mid-request, and sent again under its key, hears what it did (cl-885): a create, an import and an apply. Each is
held inside its transaction by another program's hold on project.db while its connection is closed, then let go and sent again. And a
write's key is never in project.db without its answer, from the first moment it can be read.
    python3 keyed.py <project> <out>: $COZY the cozy under test. Prints what it saw as JSON and exits 1 if anything failed."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import struct
import subprocess
import sys
import threading
import time
from urllib.parse import quote, urlsplit
import uuid
import zlib

Json = dict[str, object]
# Long enough that Calcifer is waiting on project.db when the connection goes; well inside its 10 s wait for another program.
HOLD_S = 1.5


def png() -> bytes:
    """An 8x8 picture, one colour."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    rows = (b"\x00" + bytes((40, 160, 90)) * 8) * 8
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def main() -> int:
    project, out = Path(sys.argv[1]), Path(sys.argv[2])
    failures: list[str] = []
    seen: dict[str, object] = {}

    def check(name: str, ok: bool, detail: object = None) -> None:
        seen[name] = True if ok else (detail if detail is not None else False)
        if not ok:
            failures.append(name)

    made = subprocess.run([os.environ["COZY"], "new", str(project), "--unencrypted"], cwd=out, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    if made.returncode != 0:
        print(json.dumps({"failures": ["new"], "seen": made.stderr[-1000:]}))
        return 1
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    address = urlsplit(f"http://{holder['address']}")
    host, port = address.hostname or "127.0.0.1", address.port or 80
    at = f"/v1/projects/{quote(str(project.absolute()), safe='')}"

    def raw(method: str, path: str, body: bytes, key: str | None, content_type: str = "application/json") -> bytes:
        head = [f"{method} {path} HTTP/1.1", f"Host: {host}:{port}", f"Authorization: Bearer {holder['token']}", "Cozy-Client: page e2e-keyed",
                f"Content-Type: {content_type}", f"Content-Length: {len(body)}", "Connection: close"]
        if key:
            head.append(f"Idempotency-Key: {key}")
        return ("\r\n".join(head) + "\r\n\r\n").encode() + body

    def ask(method: str, path: str, body: bytes, key: str | None, content_type: str = "application/json") -> tuple[int, dict[str, str], bytes]:
        with socket.create_connection((host, port), timeout=60) as conn:
            conn.sendall(raw(method, path, body, key, content_type))
            answer = b""
            while chunk := conn.recv(65536):
                answer += chunk
        head, _, rest = answer.partition(b"\r\n\r\n")
        lines = head.decode("latin-1").split("\r\n")
        headers = {name.strip().lower(): value.strip() for name, _, value in (line.partition(":") for line in lines[1:])}
        if headers.get("transfer-encoding") == "chunked":
            body_out, data = b"", rest
            while data:
                size, _, data = data.partition(b"\r\n")
                n = int(size, 16)
                if n == 0:
                    break
                body_out, data = body_out + data[:n], data[n + 2:]
            rest = body_out
        return int(lines[0].split()[1]), headers, rest

    def cut_off(method: str, path: str, body: bytes, key: str) -> None:
        """Sent, and its connection closed while project.db is held: Calcifer is inside the write, its asker gone."""
        db = sqlite3.connect(project / "project.db", isolation_level=None, timeout=30)
        db.execute("BEGIN IMMEDIATE")
        try:
            conn = socket.create_connection((host, port), timeout=60)
            conn.sendall(raw(method, path, body, key))
            time.sleep(HOLD_S)
            conn.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            conn.close()
        finally:
            db.execute("ROLLBACK")
            db.close()

    def first_sight(key: str, stop: threading.Event, found: dict[str, object]) -> None:
        """Its key's row as project.db first shows it: with its answer, or without."""
        db = sqlite3.connect(f"file:{project / 'project.db'}?mode=ro", uri=True, isolation_level=None, timeout=30)
        while not stop.is_set():
            row = db.execute("SELECT status, body FROM request_keys WHERE key = ?", (key,)).fetchone()
            if row:
                found["status"], found["body"] = row[0], (row[1] or b"")[:200].decode("utf-8", "replace")
                break
        db.close()

    def boxes(name: str) -> int:
        db = sqlite3.connect(f"file:{project / 'project.db'}?mode=ro", uri=True)
        try:
            return int(db.execute("SELECT count(*) FROM boxes WHERE name = ?", (name,)).fetchone()[0])
        finally:
            db.close()

    picture = png()
    sha = hashlib.sha256(picture).hexdigest()
    status, _, said = ask("PUT", f"/v1/files/sha256:{sha}", picture, None, "image/png")
    check("picture_held", status < 300, said[-300:].decode("utf-8", "replace"))

    cases: list[tuple[str, str, Json, int, str]] = [
        ("create", "/assets", {"name": "keyed_box", "exact": True, "fields": {"text": "Words a box was made with."}}, 201, "keyed_box"),
        ("import", "/imports", {"source": f"sha256:{sha}", "name": "keyed_still", "exact": True}, 200, "keyed_still"),
        ("apply", "/apply", {"nodes": {"keyed_note": {"kind": "text", "text": "Words an apply wrote."}}}, 200, "keyed_note"),
    ]
    for case, suffix, ask_body, code, name in cases:
        body = json.dumps(ask_body).encode()
        # Cut off mid-request, then sent again under its key: what it did, as it said it.
        key = uuid.uuid4().hex
        cut_off("POST", at + suffix, body, key)
        status, headers, again = ask("POST", at + suffix, body, key)
        answer: object = again.decode("utf-8", "replace")
        try:
            answer = json.loads(again)
        except ValueError:
            pass
        seen[f"{case}_again"] = {"status": status, "replayed": headers.get("idempotent-replayed"), "answer": answer}
        named = isinstance(answer, dict) and (answer.get("name") == name if case != "apply" else answer.get("applied") is True and any(
            node.get("name") == name for node in answer.get("nodes", []) if isinstance(node, dict)))
        check(f"{case}_cut_off_heard_again", status == code and headers.get("idempotent-replayed") == "true" and named, seen[f"{case}_again"])
        check(f"{case}_done_once", boxes(name) == 1, boxes(name))
        # A write's key is never in project.db without its answer, read the moment it's there.
        renamed = {**ask_body, "name": f"{name}_2"} if case != "apply" else {"nodes": {f"{name}_2": ask_body["nodes"][name]}}  # type: ignore[index]
        key, stop = uuid.uuid4().hex, threading.Event()
        found: dict[str, object] = {}
        watcher = threading.Thread(target=first_sight, args=(key, stop, found))
        watcher.start()
        status, _, said = ask("POST", at + suffix, json.dumps(renamed).encode(), key)
        time.sleep(0.5)
        stop.set()
        watcher.join()
        seen[f"{case}_first_sight"] = {"status": status, "row": found}
        check(f"{case}_key_kept_with_its_answer", status == code and found.get("status") == code, seen[f"{case}_first_sight"])

    print(json.dumps({"failures": failures, "seen": seen}, indent=1, default=str))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
