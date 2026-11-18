"""A rental's stood-in Hub (worker routes, HTTPS under a CA of its own) and provider (vast.ai's instance DELETE), for
e2e/cli/rentalself.sh (cl-977). Each request is one line of <dir>/events.jsonl.

  standin.py serve <dir> <case>   serves until killed; <dir>/ready holds "<hub port> <provider port> <CA DER base64url>"
  standin.py verdict <dir> <case> prints one line per thing the machine got wrong

The Hub answers the machine's key reads with a 4 s lease (a check-in every 2 s) and refuses its release with worker.ended, so the
machine must end itself. The provider refuses the end twice (503), then takes the pod: it kills the machine's process group
(<dir>/machine.pid, its group leader), as a provider ending the container does.

Case `held` (e2e/cli/holdwins.sh, cl-1196): a rental this computer ordered, whose lease grants the keys in <dir>/keys (one a line),
as a Hub grants the devices of the account that rented it, for 10 min. With <dir>/link_floor.json its lease names that floor for
the machine's first download, and a slow link the machine reports is answered `replacing` (rentalself.sh's slow_link, th-405).

  standin.py link <dir> slow|paced  prints one line per thing that rental got wrong of its link, <dir>/link_mbit Mbit/s wide"""

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


def serve(dir: str, case: str) -> None:
    events = open(os.path.join(dir, "events.jsonl"), "a", buffering=1)
    lock = threading.Lock()

    def note(kind: str, **fields: object) -> None:
        with lock:
            events.write(json.dumps({"at": time.time(), "kind": kind, **fields}) + "\n")

    def answer(handler: http.server.BaseHTTPRequestHandler, status: int, body: object = None) -> None:
        raw = b"" if body is None else json.dumps(body).encode()
        handler.send_response(status)
        handler.send_header("Content-Type", "application/json")
        handler.send_header("Content-Length", str(len(raw)))
        handler.end_headers()
        handler.wfile.write(raw)

    class Hub(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format: str, *args: object) -> None:
            pass

        def body(self) -> bytes:
            return self.rfile.read(int(self.headers.get("Content-Length") or 0))

        def do_GET(self) -> None:
            if self.path == "/v1/worker/rental/authorized-keys":
                worker = self.headers.get("X-Cozy-Worker-ID", "")
                note("check_in", worker=worker)
                if case == "held":
                    keys = open(os.path.join(dir, "keys")).read().split()
                    lease: dict[str, object] = {"worker_id": worker, "authorized_keys": keys, "lease_seconds": 600}
                    floor = os.path.join(dir, "link_floor.json")
                    if os.path.exists(floor):
                        lease["link_floor"] = json.load(open(floor))
                    return answer(self, 200, lease)
                return answer(self, 200, {"worker_id": worker, "authorized_keys": [], "lease_seconds": LEASE_SECONDS})
            answer(self, 404, {"error": {"code": "worker.route_not_found", "message": self.path, "remedy": "none"}})

        def do_POST(self) -> None:
            sent = self.body()
            if self.path == "/v1/worker/rental/release":
                note("release")
                return answer(self, 401, {"error": {"code": "worker.ended", "message": "the rental behind this worker has ended",
                                                    "remedy": "a pod whose rental ended holds no lease; rent a new one"}})
            if self.path == "/v1/worker/rental/speed":
                report = json.loads(sent or b"{}")
                note("speed", **{signal: report[signal] for signal in ("download", "slow_link") if signal in report})
                if "slow_link" in report:
                    return answer(self, 200, {"replacing": True})
                return answer(self, 204)
            answer(self, 404, {"error": {"code": "worker.route_not_found", "message": self.path, "remedy": "none"}})

    asks = [0]

    class Provider(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format: str, *args: object) -> None:
            pass

        def do_DELETE(self) -> None:
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

    def openssl(*args: str) -> None:
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


def verdict(dir: str, case: str) -> None:
    raw = [json.loads(line) for line in open(os.path.join(dir, "events.jsonl"))]
    # (kind, when) of each request, in order.
    events: list[tuple[str, float]] = [(str(event["kind"]), float(event["at"])) for event in raw]
    ends = [at for kind, at in events if kind == "end"]
    taken = [at for kind, at in events if kind == "taken"]
    exited = os.path.exists(os.path.join(dir, "exited"))
    wrong: list[str] = []
    if not taken:
        wrong.append(f"{case}: the pod stopped without its provider taking it ({len(ends)} provider asks, "
                     f"{'exited on its own' if exited else 'still running'}): it may still bill")
    elif len(ends) != REFUSALS + 1:
        wrong.append(f"{case}: {len(ends)} provider asks before it was taken, want {REFUSALS + 1} (two refused, then taken)")
    if case == "idle-ended":
        releases = [at for kind, at in events if kind == "release"]
        if not releases:
            wrong.append("idle-ended: it never asked its Hub to release it")
        elif ends and ends[0] < releases[0]:
            wrong.append("idle-ended: it asked its provider to end it before its Hub refused the release")
        check_ins = [at for kind, at in events if kind == "check_in"]
        last = taken[0] if taken else events[-1][1]
        gaps = [b - a for a, b in zip(check_ins, check_ins[1:])] + ([last - check_ins[-1]] if check_ins else [])
        if len(check_ins) < 4:
            wrong.append(f"idle-ended: {len(check_ins)} check-ins over its life, want one each {LEASE_SECONDS // 2} s")
        elif max(gaps) > LEASE_SECONDS:
            wrong.append(f"idle-ended: a {max(gaps):.1f} s gap between check-ins, more than its {LEASE_SECONDS} s lease")
        if releases and not any(at > releases[0] for at in check_ins):
            wrong.append("idle-ended: no check-in after it released itself, while it waited on its provider")
    for line in wrong:
        print(line)


def link(dir: str, case: str) -> None:
    floor = json.load(open(os.path.join(dir, "link_floor.json")))
    floor_mbps, window = float(floor["mbps"]), float(floor["window_seconds"])
    wide = float(open(os.path.join(dir, "link_mbit")).read())
    # (bytes, seconds) of each report, by signal.
    told: dict[str, list[tuple[float, float]]] = {"slow_link": [], "download": []}
    for line in open(os.path.join(dir, "events.jsonl")):
        event = json.loads(line)
        for signal, reports in told.items():
            if event["kind"] == "speed" and signal in event:
                reports.append((float(event[signal]["bytes"]), float(event[signal]["seconds"])))
    slow, measured = told["slow_link"], told["download"]

    def mbps(report: tuple[float, float]) -> float:
        return report[0] * 8 / 1e6 / report[1]

    said = "; ".join(f"{signal} {size / 1e6:.0f} MB over {seconds:.1f} s ({mbps((size, seconds)):.0f} Mbit/s)"
                     for signal, reports in told.items() for size, seconds in reports) or "nothing"
    wrong: list[str] = []
    if case == "slow":
        if len(slow) != 1 or measured:
            wrong.append(f"through {wide:.0f} Mbit/s, under its lease's {floor_mbps:.0f}, it told its Hub {said}: want one slow_link alone")
        elif slow[0][1] < window or not 0.5 * wide <= mbps(slow[0]) <= 1.25 * wide:
            wrong.append(f"its slow_link says {said}: want the {wide:.0f} Mbit/s link's pace over its lease's {window:.0f} s window or more")
        if "the Hub is placing this rental on another host" not in open(os.path.join(dir, "machine.log"), errors="replace").read():
            wrong.append("its log doesn't say the Hub is placing it on another host, as the Hub answered")
    else:
        if slow or len(measured) != 1:
            wrong.append(f"through {wide:.0f} Mbit/s, over its lease's {floor_mbps:.0f}, it told its Hub {said}: want one download alone")
        elif measured[0][0] < 1 << 30 or not floor_mbps <= mbps(measured[0]) <= 1.25 * wide:
            wrong.append(f"its download says {said}: want 1 GiB or more at the {wide:.0f} Mbit/s link's pace")
    for found in wrong:
        print(found)
    if not wrong:
        print(f"ok: {said}", file=sys.stderr)


if __name__ == "__main__":
    {"serve": serve, "verdict": verdict, "link": link}[sys.argv[1]](sys.argv[2], sys.argv[3])
