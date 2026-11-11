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

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_hub import require_test_hub


def b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def encoded(value: object) -> str:
    return b64(json.dumps(value, separators=(",", ":")).encode())


class Fixture:
    def __init__(self, args: argparse.Namespace):
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
        machine = json.loads(self.cli("machine", "show", "local"))
        self.address = urllib.parse.urlsplit(machine["address"])
        assert self.address.hostname and ipaddress.ip_address(self.address.hostname).is_loopback
        self.fingerprint = machine["fingerprint"].removeprefix("sha256:")
        status = json.loads(self.machine("GET", "/v1/status", authorized=False))
        self.workload = status["workload_key"]
        grant = json.dumps({"m": status["worker_id"], "a": "machine", "e": int(time.time()) + 1800,
                            "k": b64(hashlib.sha256(self.machine_public).digest()[:16])}).encode()
        self.control = b64(grant) + "." + b64(self.key.sign(b"cozy-capability/1\0" + grant))
        self.metadata = self.http("/.well-known/oauth-protected-resource")
        self.origin = self.metadata["resource"]
        issuer = self.metadata["authorization_servers"][0]
        server = self.http(issuer + "/.well-known/oauth-authorization-server")
        self.token_path = urllib.parse.urlsplit(server["token_endpoint"]).path
        self.ca = base64.b64encode(ssl.PEM_cert_to_DER_cert(Path(os.environ["COZY_TEST_HUB_CA"]).read_text())).decode()

    def cli(self, *args: str) -> str:
        return subprocess.check_output([self.args.cozy, "--json", *args], text=True)

    def admin(self, account: str, action: str):
        command = [word.decode() for word in Path(os.environ["COZY_TEST_HUB_COMMAND_FILE"]).read_bytes().split(b"\0") if word]
        subprocess.check_output([*command, "admin", action, account])

    def http(self, path: str, bearer: str = "") -> dict:
        url = path if path.startswith("http") else self.hub + path
        request = urllib.request.Request(url, headers={"Authorization": "Bearer " + bearer} if bearer else {})
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(
            context=ssl.create_default_context(cafile=os.environ["COZY_TEST_HUB_CA"])))
        with opener.open(request, timeout=60) as response:
            return json.load(response)

    def credentials(self) -> tuple[dict, dict]:
        prefix = Path.home() / ".cozy/keys" / ("device-" + hashlib.sha256(self.hub.encode()).hexdigest())
        key = json.loads(Path(str(prefix) + ".json").read_text())
        bearer = json.loads(Path(str(prefix) + ".bearer.json").read_text())["token"]
        key["bearer"] = bearer
        return key, self.http("/v1/accounts/current", bearer)

    def capability(self, credentials: tuple[dict, dict], details: list[dict]) -> str:
        key, account = credentials
        seed = base64.urlsafe_b64decode(key["private_key"] + "=" * (-len(key["private_key"]) % 4))
        claims = {"sub": account["user_id"], "aud": self.origin, "cnf": {"jkt": self.workload},
                  "authorization_details": details, "iat": int(time.time()), "exp": int(time.time()) + 3600,
                  "jti": uuid.uuid4().hex}
        value = encoded({"alg": "EdDSA", "typ": "authkit-capability+jwt", "kid": key["device_key_id"]}) + "." + encoded(claims)
        return value + "." + b64(Ed25519PrivateKey.from_private_bytes(seed).sign(value.encode()))

    def machine(self, method: str, path: str, value: object = None, *, authorized: bool = True, stream: bool = False):
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        tls.check_hostname = False
        tls.verify_mode = ssl.CERT_NONE
        connection = http.client.HTTPSConnection(self.address.hostname, self.address.port, context=tls, timeout=300)
        connection.connect()
        assert hashlib.sha256(connection.sock.getpeercert(binary_form=True)).hexdigest() == self.fingerprint
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

    def trace_rows(self) -> list[dict]:
        return [json.loads(line) for line in self.trace.read_text().splitlines() if line]

    def warm(self, label: str, *, release: str = "", checkpoint: str = "", capability: str = "",
             model: str = "", lane: str = "f16", succeeds: bool, redemption_denied: bool = False,
             cached: bool = False) -> dict:
        before = len(self.trace_rows())
        ident = "private-cache-" + uuid.uuid4().hex
        choice = {"parameter": "model", "repository": model or self.args.model, "lane": lane}
        choice.update({"manifest": checkpoint} if checkpoint else {"release": release})
        hub = {"origin": self.origin, "ca_der": self.ca}
        if capability:
            hub["capability"] = capability
        answer = json.loads(self.machine("POST", "/v1/runs/batch", {"runs": [
            {"id": ident, "spec": {"kind": "warm", "models": [choice], "hub": hub}}]}))
        admitted = answer["runs"][0]
        assert not admitted.get("refusal"), (label, admitted)
        connection, response = self.machine("GET", "/v1/runs/" + ident + "/events", stream=True)
        outcome = None
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
        digest = checkpoint.removeprefix("sha256:")
        path = (f"/blobs/{digest[:2]}/{digest[2:4]}/{digest}" if checkpoint else
                "/v1/models/" + (model or self.args.model) + "/resolve")
        reads = [row for row in traffic if row["method"] in ("GET", "HEAD") and row["path"] == path]
        if cached:
            assert not reads, (label, "public answer did not take its cache", traffic)
        else:
            assert any((200 <= row["status"] < 400) if succeeds else row["status"] in (403, 404)
                       for row in reads), (label, "no matching origin authorization result", traffic)
        if redemption_denied:
            assert any(row["method"] == "POST" and row["path"] == self.token_path and row["status"] == 400
                       for row in traffic), (label, "changed subject did not cause an actual refused redemption", traffic)
        print("PASS", label)
        return outcome

    def save(self):
        (self.out / "private-cache.json").write_text(json.dumps({
            "model": self.args.model, "hub_source": self.args.hub_source, "cases": self.cases,
        }, indent=2) + "\n")

    def login(self, email: str, name: str = ""):
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

    def restricted(self):
        owner = self.credentials()
        broad = [{"type": "tensorhub_model_read", "model": self.args.model}]
        roots = sorted(set(self.args.a_checkpoints.split(",")))
        assert self.args.b_checkpoint not in roots, "B must be outside A's frozen complete root set"
        frozen = [{"type": "tensorhub_release_read", "model": self.args.model, "release": self.args.a_version,
                   "checkpoints": roots}]
        self.warm("owner_warms_B_by_name", release=self.args.b_version, capability=self.capability(owner, broad), succeeds=True)
        self.warm("owner_warms_B_by_hash", checkpoint=self.args.b_checkpoint, capability=self.capability(owner, broad), succeeds=True)
        self.warm("A_only_denies_warm_B_name", release=self.args.b_version, capability=self.capability(owner, frozen), succeeds=False)
        self.warm("A_only_denies_warm_B_hash", checkpoint=self.args.b_checkpoint, capability=self.capability(owner, frozen), succeeds=False)
        self.warm("A_only_still_reads_A_name", release=self.args.a_version, capability=self.capability(owner, frozen), succeeds=True)
        self.warm("A_only_still_reads_A_hash", checkpoint=self.args.a_checkpoints.split(",")[0], capability=self.capability(owner, frozen), succeeds=True)
        self.warm("anonymous_denies_warm_B_name", release=self.args.b_version, succeeds=False)
        self.warm("anonymous_denies_warm_B_hash", checkpoint=self.args.b_checkpoint, succeeds=False)
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
            assert not self.http("/v1/accounts/current", other[0]["bearer"]).get("admin", False)
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

    def public(self):
        self.warm("public_anonymous_origin_positive", release=self.args.a_version, succeeds=True)
        self.warm("public_anonymous_cache_positive", release=self.args.a_version, succeeds=True, cached=True)
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
            private = self.http("/v1/models/" + self.args.model + "/resolve?" + query, owner[0]["bearer"])
            assert private["commit"]["sha256"] == self.args.b_checkpoint.removeprefix("sha256:")
            private.setdefault("reasons", [])
            private["hardware"] = public["hardware"]
            key[0], key[3] = "resolve-v4", self.args.b_version
            legacy = json.dumps(key, separators=(",", ":"))
            connection.execute("INSERT OR REPLACE INTO held_names(name,named,read_ms) VALUES(?,?,?)",
                               (legacy, json.dumps(private), int(time.time() * 1000)))
            self.cases["legacy_v4_seed"] = {"key": key, "checkpoint": self.args.b_checkpoint,
                                             "source": "actual owner origin response and actual v5 hardware"}
        self.warm("legacy_v4_private_answer_is_quarantined", release=self.args.b_version, succeeds=False)

    def close(self):
        self.cli("machine", "authorize", b64(self.machine_public), "--remove")


def main():
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
