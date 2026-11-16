#!/usr/bin/env python3
"""A lock queued on an encrypted project waits for nothing long, and holds up no other project (cl-1025).

On this computer's Calcifer, through its HTTP API as a client sends it:
1. Project A, encrypted and unlocked, takes a slow upload: one body sent over about 20 s.
2. While it's going, thumbnails of A's pictures are asked for, so the drawers have A's to do, and `cozy vault lock` is run on A.
3. Meanwhile project B, a plain one, takes a picture and its thumbnail is asked for until it's there.
The lock locks, with no `project_busy` after waiting out the upload; B's thumbnail comes at once, not after A's lock lets go; and the
upload is answered (refused once A is locked, or done). Prints what it measured as JSON; exits 1 when a check fails.
"""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import struct
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import cast
import zlib

HOLDER = Path.home() / ".cozy/calcifer/calcifer.lock.holder"
UPLOAD_BYTES = 3 * 1024 * 1024
UPLOAD_SECONDS = 20.0
CHUNK = 32 * 1024
# What the fix allows: the lock has the project as soon as the upload's next piece is written, and B's drawers never wait on A.
LOCK_WITHIN_S = 8.0
THUMB_WITHIN_S = 6.0


def holder() -> dict[str, str]:
    return cast(dict[str, str], json.loads(HOLDER.read_text()))


def headers(token: str, **more: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {token}", "cozy-project-storage": "3", **more}


def png(i: int, side: int = 320) -> bytes:
    """A picture of its own: a gradient `i` tells from the others."""
    rows = b"".join(b"\x00" + bytes(v for x in range(side) for v in ((x * 3 + i * 41) & 255, (y * 2 + i * 13) & 255, (x + y + i * 7) & 255)) for y in range(side))
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b"")


def call(method: str, path: str, body: bytes | None = None, kind: str = "application/json", timeout: float = 60) -> tuple[int, dict[str, object]]:
    held = holder()
    request = urllib.request.Request(f"http://{held['address']}{path}", data=body, method=method, headers=headers(held["token"], **({"Content-Type": kind} if body is not None else {})))
    try:
        with urllib.request.urlopen(request, timeout=timeout) as answer:
            raw = answer.read()
            return answer.status, (cast(dict[str, object], json.loads(raw)) if raw and answer.headers.get("Content-Type", "").startswith("application/json") else {})
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            return error.code, cast(dict[str, object], json.loads(raw))
        except ValueError:
            return error.code, {"raw": raw[:200].decode(errors="replace")}


def project_path(directory: Path) -> str:
    return "/v1/projects/" + urllib.parse.quote(str(directory.resolve()), safe="")


def scope(directory: Path) -> str:
    return "?project=" + urllib.parse.quote(str(directory.resolve()), safe="")


def imported(directory: Path, name: str, data: bytes) -> str:
    """`data` held by `directory`'s project and imported as box `name`: its sha256."""
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    status, said = call("PUT", f"/v1/project-files/{digest}{scope(directory)}", data, "application/octet-stream")
    assert status in (200, 201), f"upload of {name}: {status} {said}"
    status, said = call("POST", f"{project_path(directory)}/imports", json.dumps({"source": digest, "name": name}).encode())
    assert status == 200, f"import of {name}: {status} {said}"
    return digest


class Upload:
    """What the slow upload was answered, and when: set by its thread."""

    started = threading.Event()
    answer: dict[str, object] | None = None


def slow_upload(directory: Path, upload: Upload) -> None:
    """One body sent over `UPLOAD_SECONDS`, as a slow network sends it: what it was answered, and when."""
    data = os.urandom(UPLOAD_BYTES)
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    held = holder()
    host, port = held["address"].rsplit(":", 1)
    connection = http.client.HTTPConnection(host, int(port), timeout=120)
    began = time.monotonic()
    try:
        connection.putrequest("PUT", f"/v1/project-files/{digest}{scope(directory)}")
        for key, value in headers(held["token"], **{"Content-Type": "application/octet-stream", "Content-Length": str(len(data))}).items():
            connection.putheader(key, value)
        connection.endheaders()
        pause = UPLOAD_SECONDS / (len(data) / CHUNK)
        upload.started.set()
        for at in range(0, len(data), CHUNK):
            connection.send(data[at:at + CHUNK])
            time.sleep(pause)
        answer = connection.getresponse()
        upload.answer = {"status": answer.status, "after_s": round(time.monotonic() - began, 2), "said": answer.read()[:300].decode(errors="replace")}
    except (OSError, http.client.HTTPException) as error:
        # Refused while its body still went: the connection ends under it.
        upload.answer = {"ended": type(error).__name__, "after_s": round(time.monotonic() - began, 2)}
    finally:
        upload.started.set()
        connection.close()


class Lock:
    """`cozy vault lock`'s exit, how long it took, and what it said: set by its thread."""

    code: int | None = None
    after_s = 0.0
    said = ""


def state_of(said: str) -> str | None:
    """A vault answer's state, or its error's code."""
    try:
        answer = json.loads(said or "{}")
    except ValueError:
        return None
    if not isinstance(answer, dict):
        return None
    for key, field in (("vault", "state"), ("error", "code")):
        part = answer.get(key)
        if isinstance(part, dict) and isinstance(part.get(field), str):
            return cast(str, part[field])
    return None


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project-a", type=Path, required=True)
    parser.add_argument("--project-b", type=Path, required=True)
    parser.add_argument("--cli", default="cozy")
    args = parser.parse_args()
    a, b, cli = cast(Path, args.project_a), cast(Path, args.project_b), cast(str, args.cli)

    a_pictures = [imported(a, f"a_pic_{i}", png(i)) for i in range(4)]
    imported(b, "b_first", png(50))

    upload = Upload()
    uploading = threading.Thread(target=slow_upload, args=(a, upload), daemon=True)
    uploading.start()
    upload.started.wait(timeout=60)
    time.sleep(1.5)

    # A's drawers' work: each of its pictures at widths no one asked for yet, each drawn first.
    for digest in a_pictures:
        for width in (96, 112, 128, 144):
            call("GET", f"{project_path(a)}/thumbs/{digest}?w={width}")

    lock = Lock()
    def locking() -> None:
        began = time.monotonic()
        done = subprocess.run([cli, "--json", "vault", "lock"], cwd=a, capture_output=True, text=True, timeout=90)
        lock.code, lock.after_s, lock.said = done.returncode, round(time.monotonic() - began, 2), done.stdout[-600:]
    locker = threading.Thread(target=locking, daemon=True)
    locker.start()
    time.sleep(0.3)

    # B goes on: a new picture, and its thumbnail asked for until it's there.
    began = time.monotonic()
    digest = imported(b, "b_new", png(77))
    status = 0
    while time.monotonic() - began < 30:
        status, _ = call("GET", f"{project_path(b)}/thumbs/{digest}?w=200")
        if status == 200:
            break
        time.sleep(0.1)
    thumb_s = round(time.monotonic() - began, 2)

    locker.join(timeout=90)
    uploading.join(timeout=90)
    lock_state = state_of(lock.said)
    failed = []
    if lock.code != 0 or lock_state != "locked":
        failed.append(f"the lock didn't lock beside a slow upload: {lock_state}")
    elif lock.after_s > LOCK_WITHIN_S:
        failed.append(f"the lock waited {lock.after_s} s for the upload")
    if status != 200 or thumb_s > THUMB_WITHIN_S:
        failed.append(f"B's thumbnail waited on A's lock: {status} after {thumb_s} s")
    if upload.answer is None:
        failed.append("the slow upload was never answered")
    seen = {
        "lock": {"code": lock.code, "after_s": lock.after_s, "said": lock.said},
        "lock_state": lock_state,
        "b_thumb": {"status": status, "after_s": thumb_s},
        "upload": upload.answer,
        "failed": failed,
    }
    print(json.dumps(seen, indent=2))
    raise SystemExit(1 if failed else 0)


if __name__ == "__main__":
    main()
