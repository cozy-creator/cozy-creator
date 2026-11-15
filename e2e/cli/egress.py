"""e2e/cli/egress.sh's client of the local machine (xs-196 D): requests as any client holding the machine's authorization
sends them, past Calcifer's own checks, each run's refusal or outcome written to OUT as JSON.

  egress.py models PEM ADDRESS FINGERPRINT OUT REDIRECT SECRET   model sources: a machine path, an added path, the
                                                                 metadata address, a redirect to it, a presigned 404
  egress.py media PEM ADDRESS FINGERPRINT OUT PACKAGE NAME=FILE…  each FILE as the clip of PACKAGE's `probe`, sent as
                                                                 video/mp4 (Calcifer's own sniff never sees it)
"""

import base64
import hashlib
import http.client
import json
import os
import ssl
import sys
import time
from typing import Any

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import load_pem_private_key

command, pem, address, fingerprint, out, *rest = sys.argv[1:]
key = load_pem_private_key(open(pem, "rb").read(), None)
assert isinstance(key, Ed25519PrivateKey), f"{pem} is no Ed25519 key"
host, port = address.split("://")[-1].rsplit(":", 1)
tls = ssl.create_default_context()
tls.check_hostname, tls.verify_mode = False, ssl.CERT_NONE


def b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def connect(timeout: int = 60) -> http.client.HTTPSConnection:
    connection = http.client.HTTPSConnection(host.strip("[]"), int(port), context=tls, timeout=timeout)
    connection.connect()
    peer = connection.sock.getpeercert(binary_form=True)
    assert isinstance(peer, bytes), f"{address} presents no certificate"
    leaf = hashlib.sha256(peer).hexdigest()
    assert leaf == fingerprint.removeprefix("sha256:"), f"{address} presents the leaf {leaf}, not {fingerprint}"
    return connection


connection = connect()
connection.request("GET", "/v1/status")
worker = json.loads(connection.getresponse().read())["worker_id"]
grant = json.dumps({"m": worker, "a": "machine", "e": int(time.time()) + 1800,
                    "k": b64(hashlib.sha256(key.public_key().public_bytes_raw()).digest()[:16])}).encode()
domain = b"cozy-capability/1\0"  # api::machine::capability's DOMAIN, then the grant
token = f"{b64(grant)}.{b64(key.sign(domain + grant))}"


def call(method: str, path: str, body: bytes = b"") -> tuple[int, bytes]:
    connection = connect()
    connection.request(method, path, body=body, headers={"content-length": str(len(body)), "authorization": f"Cozy-Cap {token}"})
    answer = connection.getresponse()
    return answer.status, answer.read()


def put(data: bytes) -> str:
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    status, body = call("PUT", f"/v1/objects/{digest}", data)
    assert status < 300, f"PUT /v1/objects/{digest}: {status} {body[:300]!r}"
    return digest


def outcome(run: str) -> dict[str, Any]:
    """The run's outcome, read from its event stream."""
    connection = connect(timeout=900)
    connection.request("GET", f"/v1/runs/{run}/events", headers={"authorization": f"Cozy-Cap {token}", "accept": "text/event-stream"})
    answer, event = connection.getresponse(), ""
    for raw in answer:
        line = raw.decode().rstrip("\n")
        if line.startswith("event:"):
            event = line[6:].strip()
        elif line.startswith("data:") and event == "outcome":
            said: dict[str, Any] = json.loads(line[5:])["event"]["outcome"]
            return said
    return {"status": "no outcome"}


def batch(specs: dict[str, dict[str, Any]]) -> dict[str, Any]:
    """Each spec sent as one run; its refusal, or the outcome it came to."""
    stamp, said = time.time_ns(), dict[str, Any]()
    runs = [{"id": f"e2e-egress-{name}-{stamp}", "spec": spec} for name, spec in specs.items()]
    status, body = call("POST", "/v1/runs/batch", json.dumps({"runs": runs}).encode())
    assert status == 200, f"POST /v1/runs/batch: {status} {body[:300]!r}"
    for name, admitted in zip(specs, json.loads(body)["runs"]):
        said[name] = {"refusal": admitted["refusal"]} if admitted.get("refusal") else {"outcome": outcome(f"e2e-egress-{name}-{stamp}")}
        print(f"{name}: {json.dumps(said[name])[:400]}")
    return said


if command == "models":
    redirect, secret = rest

    def warm(*models: dict[str, str]) -> dict[str, Any]:
        return {"kind": "warm", "models": list(models)}

    said = batch({
        "path": warm({"parameter": "model", "source": "/etc/hostname"}),
        "added": warm({"parameter": "model", "source": "hf://cozy-e2e/none"}, {"parameter": "add:transformer/notes.txt", "source": "/etc/hostname"}),
        "metadata": warm({"parameter": "model", "source": "https://169.254.169.254/latest/meta-data/model.safetensors"}),
        "redirect": warm({"parameter": "model", "source": f"{redirect}/model.safetensors"}),
        "presigned": warm({"parameter": "model", "source": f"{redirect}/missing.safetensors?X-Amz-Signature={secret}"}),
    })
elif command == "media":
    package, *inputs = rest
    files: list[dict[str, Any]] = []
    for root, dirs, names in os.walk(package):
        dirs[:] = [d for d in dirs if not d.startswith(".") and d != "__pycache__"]
        for name in sorted(n for n in names if not n.startswith(".")):
            path = os.path.join(root, name)
            data = open(path, "rb").read()
            files.append({"name": os.path.relpath(path, package), "digest": put(data), "length": len(data)})
    manifest = put(json.dumps({"package": "local/e2e-clip", "release": "0.1.0", "files": files, "project": ""}).encode())
    specs: dict[str, dict[str, Any]] = {}
    for named in inputs:
        name, path = named.split("=", 1)
        data = open(path, "rb").read()
        digest = put(data)
        specs[name] = {"kind": "call", "source": {"local": {"manifest": manifest}}, "entrypoint": "probe", "payload": {"clip": digest},
                       "inputs": [{"field": "clip", "digest": digest, "length": len(data), "media_type": "video/mp4", "order": 0}]}
    said = batch(specs)
else:
    sys.exit(f"no command {command}")
open(out, "w").write(json.dumps(said))
