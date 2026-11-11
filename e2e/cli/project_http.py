"""Authenticated local HTTP for the threaded project database fixtures."""
from http.client import HTTPConnection
import json
from pathlib import Path
from urllib.parse import quote, urlsplit
from uuid import uuid4


def project_path(project: str | Path, suffix: str = "") -> str:
    return f"/v1/projects/{quote(str(Path(project).absolute()), safe='')}{suffix}"


class Refused(Exception):
    def __init__(self, status: int, payload: str):
        super().__init__(f"HTTP {status}: {payload}")
        self.status = status
        self.payload = payload


def request(path: str, method: str = "GET", body: dict | bytes | None = None, *,
            edit: str | None = None) -> str:
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    address = urlsplit(f"http://{holder['address']}")
    assert address.hostname in {"127.0.0.1", "::1", "localhost"} and address.port
    headers = {"Authorization": f"Bearer {holder['token']}", "Cozy-Client": "page e2e-db"}
    if method != "GET":
        headers["Idempotency-Key"] = uuid4().hex
    if edit:
        headers["Cozy-Edit"] = edit
    if isinstance(body, dict):
        body = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    elif body is not None:
        headers["Content-Type"] = "application/yaml"
    connection = HTTPConnection(address.hostname, address.port, timeout=40)
    try:
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        payload = response.read().decode()
    finally:
        connection.close()
    if not 200 <= response.status < 300:
        raise Refused(response.status, payload)
    return payload
