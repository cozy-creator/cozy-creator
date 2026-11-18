"""A machine that only says its Status (cl-1107's queue quote cell, plan.sh's plan_queue): the frame a real one pushed, replayed.

`GET /v1/status` answers the frame in `--frame` (read again each time), as JSON or, asked for events, as one `status` event on a
stream kept open. Any other call is refused, so nothing is ever sent to run here. TLS with `--cert`, whose leaf Calcifer pins."""

import argparse
import json
import ssl
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    frame: Path

    def do_GET(self) -> None:
        if self.path.split("?", 1)[0] != "/v1/status":
            return self.refuse()
        frame = self.frame.read_bytes()
        if "text/event-stream" not in self.headers.get("Accept", ""):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(frame)))
            self.end_headers()
            self.wfile.write(frame)
            return None
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        try:
            self.wfile.write(b"event: status\ndata: " + json.dumps(json.loads(frame)).encode() + b"\n\n")
            self.wfile.flush()
            while True:
                time.sleep(10)
                self.wfile.write(b": still here\n\n")
                self.wfile.flush()
        except OSError:
            return None

    def refuse(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        body = json.dumps({"error": {"kind": "not_found", "code": "unknown_route", "message": f"a stand-in machine says only its Status, not {self.path}"}}).encode()
        self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_POST = do_PUT = do_PATCH = do_DELETE = refuse

    def log_message(self, format: str, *args: object) -> None:
        print(f"{self.address_string()} {format % args}", file=sys.stderr, flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cert", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--frame", type=Path, required=True)
    args = parser.parse_args()
    Handler.frame = args.frame
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(args.cert, args.key)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
