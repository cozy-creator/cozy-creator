"""A rental's stood-in Hub (worker routes, HTTPS under a CA of its own) and provider (vast.ai's instance DELETE), for
e2e/cli/rentalself.sh (cl-977). Each request is one line of <dir>/events.jsonl.

  standin.py serve <dir> <case>   serves until killed; <dir>/ready holds "<hub port> <provider port> <CA DER base64url>"
  standin.py verdict <dir> <case> prints one line per thing the machine got wrong

The Hub answers the machine's key reads with a 4 s lease (a check-in every 2 s) and refuses its release with worker.ended, so the
machine must end itself. The provider refuses the end twice (503), then takes the pod: it kills the machine's process group
(<dir>/machine.pid, its group leader), as a provider ending the container does."""

import base64
import http.server
import json
import os
import signal
import ssl
import subprocess
import sys
import threading
import time

REFUSALS = 2
LEASE_SECONDS = 4


def serve(dir, case):
    events = open(os.path.join(dir, "events.jsonl"), "a", buffering=1)
    lock = threading.Lock()

    def note(kind, **fields):
        with lock:
            events.write(json.dumps({"at": time.time(), "kind": kind, **fields}) + "\n")

    def answer(handler, status, body=None):
        raw = b"" if body is None else json.dumps(body).encode()
        handler.send_response(status)
        handler.send_header("Content-Type", "application/json")
        handler.send_header("Content-Length", str(len(raw)))
        handler.end_headers()
        handler.wfile.write(raw)

    class Hub(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def body(self):
            return self.rfile.read(int(self.headers.get("Content-Length") or 0))

        def do_GET(self):
            if self.path == "/v1/worker/rental/authorized-keys":
                worker = self.headers.get("X-Cozy-Worker-ID", "")
                note("check_in", worker=worker)
                return answer(self, 200, {"worker_id": worker, "authorized_keys": [], "lease_seconds": LEASE_SECONDS})
            answer(self, 404, {"error": {"code": "worker.route_not_found", "message": self.path, "remedy": "none"}})

        def do_POST(self):
            self.body()
            if self.path == "/v1/worker/rental/release":
                note("release")
                return answer(self, 401, {"error": {"code": "worker.ended", "message": "the rental behind this worker has ended",
                                                    "remedy": "a pod whose rental ended holds no lease; rent a new one"}})
            if self.path == "/v1/worker/rental/speed":
                return answer(self, 204)
            answer(self, 404, {"error": {"code": "worker.route_not_found", "message": self.path, "remedy": "none"}})

    asks = [0]

    class Provider(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def do_DELETE(self):
            asks[0] += 1
            if asks[0] <= REFUSALS:
                note("end", path=self.path, status=503)
                return answer(self, 503, {"success": False, "msg": "stand-in: try again"})
            note("end", path=self.path, status=200)
            answer(self, 200, {"success": True})
            self.wfile.flush()
            pid = int(open(os.path.join(dir, "machine.pid")).read().strip())
            try:
                os.killpg(pid, signal.SIGKILL)
                note("taken", pid=pid)
            except ProcessLookupError:
                note("taken_gone", pid=pid)

    tls = os.path.join(dir, "tls")
    os.makedirs(tls, exist_ok=True)

    def openssl(*args):
        subprocess.run(["openssl", *args], check=True, capture_output=True)

    openssl("req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "1", "-subj", "/CN=stand-in hub CA",
            "-keyout", f"{tls}/ca.key", "-out", f"{tls}/ca.pem", "-addext", "basicConstraints=critical,CA:TRUE", "-addext",
            "keyUsage=critical,keyCertSign")
    openssl("req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-subj", "/CN=127.0.0.1", "-keyout",
            f"{tls}/hub.key", "-out", f"{tls}/hub.csr")
    with open(f"{tls}/ext", "w") as ext:
        ext.write("subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n")
    openssl("x509", "-req", "-in", f"{tls}/hub.csr", "-CA", f"{tls}/ca.pem", "-CAkey", f"{tls}/ca.key", "-CAcreateserial", "-days",
            "1", "-extfile", f"{tls}/ext", "-out", f"{tls}/hub.pem")
    der = subprocess.run(["openssl", "x509", "-in", f"{tls}/ca.pem", "-outform", "DER"], check=True, capture_output=True).stdout

    hub = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Hub)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(f"{tls}/hub.pem", f"{tls}/hub.key")
    context.set_alpn_protocols(["http/1.1"])
    hub.socket = context.wrap_socket(hub.socket, server_side=True)
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    for server in (hub, provider):
        threading.Thread(target=server.serve_forever, daemon=True).start()
    ready = os.path.join(dir, "ready")
    with open(ready + ".tmp", "w") as out:
        out.write(f"{hub.server_address[1]} {provider.server_address[1]} {base64.urlsafe_b64encode(der).rstrip(b'=').decode()}\n")
    os.replace(ready + ".tmp", ready)
    signal.sigwait({signal.SIGTERM, signal.SIGINT})


def verdict(dir, case):
    events = [json.loads(line) for line in open(os.path.join(dir, "events.jsonl"))]
    ends = [e for e in events if e["kind"] == "end"]
    taken = [e for e in events if e["kind"] == "taken"]
    exited = os.path.exists(os.path.join(dir, "exited"))
    wrong = []
    if not taken:
        wrong.append(f"{case}: the pod stopped without its provider taking it ({len(ends)} provider asks, "
                     f"{'exited on its own' if exited else 'still running'}): it may still bill")
    elif len(ends) != REFUSALS + 1:
        wrong.append(f"{case}: {len(ends)} provider asks before it was taken, want {REFUSALS + 1} (two refused, then taken)")
    if case == "idle-ended":
        releases = [e for e in events if e["kind"] == "release"]
        if not releases:
            wrong.append("idle-ended: it never asked its Hub to release it")
        elif ends and ends[0]["at"] < releases[0]["at"]:
            wrong.append("idle-ended: it asked its provider to end it before its Hub refused the release")
        check_ins = [e["at"] for e in events if e["kind"] == "check_in"]
        last = taken[0]["at"] if taken else events[-1]["at"]
        gaps = [b - a for a, b in zip(check_ins, check_ins[1:])] + ([last - check_ins[-1]] if check_ins else [])
        if len(check_ins) < 4:
            wrong.append(f"idle-ended: {len(check_ins)} check-ins over its life, want one each {LEASE_SECONDS // 2} s")
        elif max(gaps) > LEASE_SECONDS:
            wrong.append(f"idle-ended: a {max(gaps):.1f} s gap between check-ins, more than its {LEASE_SECONDS} s lease")
        if releases and not any(at > releases[0]["at"] for at in check_ins):
            wrong.append("idle-ended: no check-in after it released itself, while it waited on its provider")
    for line in wrong:
        print(line)


if __name__ == "__main__":
    {"serve": serve, "verdict": verdict}[sys.argv[1]](sys.argv[2], sys.argv[3])
