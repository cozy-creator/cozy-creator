"""Replay restricted capabilities through a real, owned TensorD; Calcifer must not widen them."""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import ipaddress
import json
import os
from pathlib import Path
import ssl
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
import uuid
from typing import Literal, NotRequired, TypedDict, cast, overload

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_hub import require_test_hub


Object = dict[str, object]


class Account(TypedDict):
    name: str
    user_id: str


class DeviceKey(TypedDict):
    device_key_id: str
    private_key: str


class Device(DeviceKey):
    bearer: str


class HubMetadata(TypedDict):
    resource: str
    authorization_servers: list[str]


class OAuthMetadata(TypedDict):
    token_endpoint: str


class MachineDetail(TypedDict):
    address: str
    fingerprint: str


class MachineStatus(TypedDict):
    worker_id: str
    workload_key: str
    models: NotRequired[list["MachineModel"]]


class MachineModel(TypedDict):
    repository: str
    checkpoints: list["MachineCheckpoint"]


class MachineCheckpoint(TypedDict):
    manifest: str
    release: str


class ObjectRef(TypedDict):
    sha256: str
    length: int


class Resolution(TypedDict):
    release: str
    lane: str
    commit: ObjectRef
    header: ObjectRef
    reasons: NotRequired[list[str]]


class Trace(TypedDict):
    method: str
    path: str
    status: int


class Outcome(TypedDict):
    status: str
    reason: NotRequired[object]
    result: NotRequired[object]


def b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def encoded(value: object) -> str:
    return b64(json.dumps(value, separators=(",", ":")).encode())


class Fixture:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.hub = os.environ["COZY_HUB_URL"]
        require_test_hub(self.hub, os.environ["COZY_TEST_HUB_ORIGIN"])
        assert os.environ.get("GITHUB_ACTIONS") == "true", "fresh hosted runner only"
        self.out = Path(args.out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.trace = Path(os.environ["COZY_TEST_HUB_REQUEST_LOG"])
        self.cases: dict[str, object] = {}
        self.key = Ed25519PrivateKey.generate()
        self.machine_public = self.key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        self.cli("machine", "authorize", b64(self.machine_public))
        machine = cast(MachineDetail, json.loads(self.cli("machine", "show", "local")))
        self.address = urllib.parse.urlsplit(machine["address"])
        host = self.address.hostname
        assert host and ipaddress.ip_address(host).is_loopback
        self.machine_host = host
        self.fingerprint = machine["fingerprint"].removeprefix("sha256:")
        status = cast(MachineStatus, json.loads(self.machine("GET", "/v1/status", authorized=False)))
        self.workload = status["workload_key"]
        grant = json.dumps({"m": status["worker_id"], "a": "machine", "e": int(time.time()) + 1800,
                            "k": b64(hashlib.sha256(self.machine_public).digest()[:16])}).encode()
        self.control = b64(grant) + "." + b64(self.key.sign(b"cozy-capability/1\0" + grant))
        self.metadata = cast(HubMetadata, self.http("/.well-known/oauth-protected-resource"))
        self.origin = self.metadata["resource"]
        issuer = self.metadata["authorization_servers"][0]
        server = cast(OAuthMetadata, self.http(issuer + "/.well-known/oauth-authorization-server"))
        self.token_path = urllib.parse.urlsplit(server["token_endpoint"]).path
        self.ca = base64.b64encode(ssl.PEM_cert_to_DER_cert(Path(os.environ["COZY_TEST_HUB_CA"]).read_text())).decode()

    def cli(self, *args: str) -> str:
        return subprocess.check_output([self.args.cozy, "--json", *args], text=True)

    def admin(self, account: str, action: str) -> None:
        command = [word.decode() for word in Path(os.environ["COZY_TEST_HUB_COMMAND_FILE"]).read_bytes().split(b"\0") if word]
        subprocess.check_output([*command, "admin", action, account])

    def http(self, path: str, bearer: str = "") -> Object:
        url = path if path.startswith("http") else self.hub + path
        request = urllib.request.Request(url, headers={"Authorization": "Bearer " + bearer} if bearer else {})
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(
            context=ssl.create_default_context(cafile=os.environ["COZY_TEST_HUB_CA"])))
        with opener.open(request, timeout=60) as response:
            return cast(Object, json.load(response))

    def credentials(self) -> tuple[Device, Account]:
        prefix = Path.home() / ".cozy/keys" / ("device-" + hashlib.sha256(self.hub.encode()).hexdigest())
        key = cast(DeviceKey, json.loads(Path(str(prefix) + ".json").read_text()))
        bearer = cast(dict[str, str], json.loads(Path(str(prefix) + ".bearer.json").read_text()))["token"]
        device = Device(device_key_id=key["device_key_id"], private_key=key["private_key"], bearer=bearer)
        return device, cast(Account, self.http("/v1/accounts/current", bearer))

    def capability(self, credentials: tuple[Device, Account], details: list[Object]) -> str:
        key, account = credentials
        seed = base64.urlsafe_b64decode(key["private_key"] + "=" * (-len(key["private_key"]) % 4))
        claims = {"sub": account["user_id"], "aud": self.origin, "cnf": {"jkt": self.workload},
                  "authorization_details": details, "iat": int(time.time()), "exp": int(time.time()) + 3600,
                  "jti": uuid.uuid4().hex}
        value = encoded({"alg": "EdDSA", "typ": "authkit-capability+jwt", "kid": key["device_key_id"]}) + "." + encoded(claims)
        return value + "." + b64(Ed25519PrivateKey.from_private_bytes(seed).sign(value.encode()))

    @overload
    def machine(self, method: str, path: str, value: object = None, *, authorized: bool = True,
                stream: Literal[False] = False) -> bytes: ...

    @overload
    def machine(self, method: str, path: str, value: object = None, *, authorized: bool = True,
                stream: Literal[True]) -> tuple[http.client.HTTPSConnection, http.client.HTTPResponse]: ...

    def machine(self, method: str, path: str, value: object = None, *, authorized: bool = True,
                stream: bool = False) -> bytes | tuple[http.client.HTTPSConnection, http.client.HTTPResponse]:
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        tls.check_hostname = False
        tls.verify_mode = ssl.CERT_NONE
        connection = http.client.HTTPSConnection(self.machine_host, self.address.port, context=tls, timeout=300)
        connection.connect()
        assert connection.sock is not None
        certificate = connection.sock.getpeercert(binary_form=True)
        assert certificate is not None
        assert hashlib.sha256(certificate).hexdigest() == self.fingerprint
        body = None if value is None else json.dumps(value).encode()
        headers = {"Content-Type": "application/json"}
        if authorized:
            headers["Authorization"] = "Cozy-Cap " + self.control
        if stream:
            headers["Accept"] = "text/event-stream"
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        assert response.status == 200, (method, path, response.status, response.read()[:300])
        if stream:
            return connection, response
        answer = response.read()
        connection.close()
        return answer

    def trace_rows(self) -> list[Trace]:
        return [cast(Trace, json.loads(line)) for line in self.trace.read_text().splitlines() if line]

    def warm(self, label: str, *, release: str = "", checkpoint: str = "", capability: str = "",
             model: str = "", lane: str = "f16", succeeds: bool, redemption_denied: bool = False,
             cached: bool = False, bare: bool = False, origin_checkpoint: str = "") -> Outcome:
        before = len(self.trace_rows())
        ident = "private-cache-" + uuid.uuid4().hex
        choice = {"parameter": "model", "lane": lane}
        if not bare:
            choice["repository"] = model or self.args.model
        choice.update({"manifest": checkpoint} if checkpoint else {"release": release})
        hub = {"origin": self.origin, "ca_der": self.ca}
        if capability:
            hub["capability"] = capability
        answer = json.loads(self.machine("POST", "/v1/runs/batch", {"runs": [
            {"id": ident, "spec": {"kind": "warm", "models": [choice], "hub": hub}}]}))
        admitted = answer["runs"][0]
        assert not admitted.get("refusal"), (label, admitted)
        connection, response = self.machine("GET", "/v1/runs/" + ident + "/events", stream=True)
        outcome: Outcome | None = None
        data = []
        try:
            while line := response.readline():
                line = line.decode().strip()
                if line.startswith("data:"):
                    data.append(line[5:].strip())
                elif not line and data:
                    event = json.loads("\n".join(data))["event"]
                    data = []
                    if "outcome" in event:
                        outcome = event["outcome"]
                        break
        finally:
            connection.close()
        traffic = self.trace_rows()[before:]
        claims = {}
        if capability:
            payload = capability.split(".")[1]
            claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        self.cases[label] = {"request": choice, "subject": claims.get("sub"),
                             "authority": claims.get("authorization_details", []),
                             "outcome": outcome, "origin_requests": traffic}
        self.save()
        assert outcome and outcome["status"] == ("succeeded" if succeeds else "failed"), (label, outcome)
        digest = (origin_checkpoint or checkpoint).removeprefix("sha256:")
        path = (f"/blobs/{digest[:2]}/{digest[2:4]}/{digest}" if checkpoint else
                "/v1/models/" + (model or self.args.model) + "/resolve")
        reads = [row for row in traffic if row["method"] in ("GET", "HEAD") and row["path"] == path]
        if cached:
            assert not reads, (label, "expected local reuse without an origin metadata read", traffic)
        else:
            assert any((200 <= row["status"] < 400) if succeeds else row["status"] in (403, 404)
                       for row in reads), (label, "no matching origin authorization result", traffic)
        if redemption_denied:
            assert any(row["method"] == "POST" and row["path"] == self.token_path and row["status"] == 400
                       for row in traffic), (label, "changed subject did not cause an actual refused redemption", traffic)
        print("PASS", label)
        return outcome

    def save(self) -> None:
        (self.out / "private-cache.json").write_text(json.dumps({
            "model": self.args.model, "hub_source": self.args.hub_source, "cases": self.cases,
        }, indent=2) + "\n")

    def login(self, email: str, name: str = "") -> None:
        current = json.loads(self.cli("whoami"))
        if current.get("logged_in") and (current.get("email") or "").lower() == email.lower():
            return
        log = Path(self.args.hub_log)
        offset = log.stat().st_size
        with tempfile.TemporaryFile() as output:
            child = subprocess.Popen([self.args.cozy, "--json", "login", email], stdin=subprocess.PIPE,
                                     stdout=output, stderr=subprocess.STDOUT)
            try:
                code = ""
                end = time.monotonic() + 120
                while child.poll() is None and time.monotonic() < end:
                    for line in log.read_bytes()[offset:].decode(errors="replace").splitlines():
                        if "development device enrollment code" in line:
                            code = json.loads(line)["code"]
                    if code:
                        break
                    time.sleep(0.2)
                assert code, "owned Hub did not produce the requested login code"
                child.communicate((code + "\n" + name + "\n").encode(), timeout=120)
                assert child.returncode == 0, "owned fixture account login failed"
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def restricted(self) -> None:
        owner = self.credentials()
        broad: list[Object] = [{"type": "tensorhub_model_read", "model": self.args.model}]
        roots = sorted(set(self.args.a_checkpoints.split(",")))
        assert self.args.b_checkpoint not in roots, "B must be outside A's frozen complete root set"
        frozen: list[Object] = [{"type": "tensorhub_release_read", "model": self.args.model, "release": self.args.a_version,
                   "checkpoints": roots}]
        self.warm("owner_warms_B_by_name", release=self.args.b_version, capability=self.capability(owner, broad), succeeds=True)
        self.warm("owner_warms_B_by_hash", checkpoint=self.args.b_checkpoint, capability=self.capability(owner, broad), succeeds=True)
        query = urllib.parse.urlencode({"release": self.args.b_version, "lane": "f16"})
        resolved = cast(Resolution, self.http("/v1/models/" + self.args.model + "/resolve?" + query, owner[0]["bearer"]))
        b_header = "sha256:" + resolved["header"]["sha256"]
        status = cast(MachineStatus, json.loads(self.machine("GET", "/v1/status")))
        assert any(row["repository"] == self.args.model and any(
            checkpoint["release"] == self.args.b_version and checkpoint["manifest"] == b_header
            for checkpoint in row["checkpoints"]) for row in status.get("models", [])), "B's version header was not cached"
        self.warm("A_only_denies_warm_B_name", release=self.args.b_version, capability=self.capability(owner, frozen), succeeds=False)
        self.warm("A_only_denies_warm_B_hash", checkpoint=self.args.b_checkpoint, capability=self.capability(owner, frozen), succeeds=False)
        self.warm("A_only_denies_warm_B_bare_header", checkpoint=b_header, bare=True,
                  origin_checkpoint=self.args.b_checkpoint, capability=self.capability(owner, frozen), succeeds=False)
        self.warm("A_only_still_reads_A_name", release=self.args.a_version, capability=self.capability(owner, frozen), succeeds=True)
        self.warm("A_only_still_reads_A_hash", checkpoint=self.args.a_checkpoints.split(",")[0], capability=self.capability(owner, frozen), succeeds=True)
        self.warm("anonymous_denies_warm_B_name", release=self.args.b_version, succeeds=False)
        self.warm("anonymous_denies_warm_B_hash", checkpoint=self.args.b_checkpoint, succeeds=False)
        self.warm("anonymous_denies_warm_B_bare_header", checkpoint=b_header, bare=True,
                  origin_checkpoint=self.args.b_checkpoint, succeeds=False)
        self.local_positive(self.capability(owner, frozen))
        other_model = ""
        try:
            self.login("cache-" + uuid.uuid4().hex + "@skew.test", "cache" + uuid.uuid4().hex[:16])
            other = self.credentials()
            assert other[1]["user_id"] != owner[1]["user_id"]
            other_model = other[1]["name"] + "/cache-positive"
            with tempfile.TemporaryDirectory() as temp:
                file = Path(temp) / "other.safetensors"
                header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
                header += b" " * (-len(header) % 8)
                file.write_bytes(struct.pack("<Q", len(header)) + header + os.urandom(32))
                self.admin(other[1]["name"], "grant")
                try:
                    self.cli("model", "upload", str(file), other_model, "--lane", "f16", "--source-format", "as-is", "--on", "local", "-y")
                finally:
                    self.admin(other[1]["name"], "revoke")
            assert self.http("/v1/accounts/current", other[0]["bearer"]).get("admin", False) is False
            # Rewarm immediately with the old owner's still-valid key; then change only the signing subject.
            self.warm("owner_rewarms_B_before_subject_switch", release=self.args.b_version, capability=self.capability(owner, broad), succeeds=True)
            self.warm("other_subject_same_scope_denied", release=self.args.b_version, capability=self.capability(other, broad),
                      succeeds=False, redemption_denied=True)
            self.warm("other_account_own_model_positive", model=other_model,
                      capability=self.capability(other, [{"type": "tensorhub_model_read", "model": other_model}]), succeeds=True)
        finally:
            try:
                if other_model:
                    self.cli("model", "delete", other_model, "-y")
            finally:
                self.login(self.args.owner_email)

    def local_positive(self, capability: str) -> None:
        alias = "local/cache-bare-" + uuid.uuid4().hex[:12]
        with tempfile.TemporaryDirectory() as temp:
            file = Path(temp) / "local.safetensors"
            header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
            header += b" " * (-len(header) % 8)
            file.write_bytes(struct.pack("<Q", len(header)) + header + os.urandom(32))
            self.cli("model", "install", str(file), alias, "--source-format", "as-is", "--on", "local")
        try:
            status = cast(MachineStatus, json.loads(self.machine("GET", "/v1/status")))
            local = [checkpoint["manifest"] for row in status.get("models", []) if row["repository"] == alias
                     for checkpoint in row["checkpoints"]]
            assert len(local) == 1, "local positive did not create one actual header"
            self.warm("bare_local_header_positive", checkpoint=local[0], bare=True,
                      capability=capability, succeeds=True, cached=True)
            self.warm("local_alias_positive", model=alias, capability=capability, succeeds=True, cached=True)
        finally:
            self.cli("model", "uninstall", alias)

    def public(self) -> None:
        self.warm("public_anonymous_origin_positive", release=self.args.a_version, succeeds=True)
        self.warm("public_anonymous_cache_positive", release=self.args.a_version, succeeds=True, cached=True)
        query_a = urllib.parse.urlencode({"release": self.args.a_version, "lane": "f16"})
        resolved_a = cast(Resolution, self.http("/v1/models/" + self.args.model + "/resolve?" + query_a))
        self.warm("public_bare_header_positive", checkpoint="sha256:" + resolved_a["header"]["sha256"], bare=True,
                  origin_checkpoint=self.args.a_checkpoints.split(",")[0], succeeds=True)
        # Replay the exact legacy format using a real anonymous answer's hardware and an
        # owner's real private-B answer. This is persisted v4 pollution, not a fake origin.
        database = Path.home() / ".cozy/machine/var/lib/cozy/tensord/execution/executions.sqlite3"
        assert database.is_file(), "fresh local fixture journal is absent"
        with sqlite3.connect(database, timeout=30) as connection:
            rows = []
            for name, body in connection.execute("SELECT name,named FROM held_names"):
                try:
                    key = json.loads(name)
                except json.JSONDecodeError:
                    continue
                if (isinstance(key, list) and key[:1] == ["resolve-v5"] and key[2:5] ==
                        [self.args.model, self.args.a_version, "f16"]):
                    rows.append((key, json.loads(body)))
            assert len(rows) == 1, "public positive did not leave exactly one actual v5 answer"
            key, public = rows[0]
            owner = self.credentials()
            query = urllib.parse.urlencode({"release": self.args.b_version, "lane": "f16"})
            private = cast(Resolution, self.http("/v1/models/" + self.args.model + "/resolve?" + query, owner[0]["bearer"]))
            assert private["commit"]["sha256"] == self.args.b_checkpoint.removeprefix("sha256:")
            legacy_answer: Object = {"release": private["release"], "lane": private["lane"], "commit": private["commit"],
                                     "header": private["header"], "reasons": private.get("reasons", []), "hardware": public["hardware"]}
            key[0], key[3] = "resolve-v4", self.args.b_version
            legacy = json.dumps(key, separators=(",", ":"))
            connection.execute("INSERT OR REPLACE INTO held_names(name,named,read_ms) VALUES(?,?,?)",
                               (legacy, json.dumps(legacy_answer), int(time.time() * 1000)))
            self.cases["legacy_v4_seed"] = {"key": key, "checkpoint": self.args.b_checkpoint,
                                             "source": "actual owner origin response and actual v5 hardware"}
        self.warm("legacy_v4_private_answer_is_quarantined", release=self.args.b_version, succeeds=False)

    def close(self) -> None:
        self.cli("machine", "authorize", b64(self.machine_public), "--remove")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("cozy", "out", "hub-log", "hub-source", "model", "a-version", "b-version", "a-checkpoints", "b-checkpoint"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--owner-email", default="ci@skew.test")
    parser.add_argument("--public", action="store_true")
    args = parser.parse_args()
    fixture = Fixture(args)
    try:
        fixture.public() if args.public else fixture.restricted()
    finally:
        fixture.close()


if __name__ == "__main__":
    main()
