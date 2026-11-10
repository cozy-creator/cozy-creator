#!/usr/bin/env python3
"""What a pod's root can make the door to its Calcifer say: door.sh names this as the home's holder, and every answer tries to
run script on the origin it is shown on, set a cookie or send the client elsewhere. Logs each request as one JSON line.
    hostile_calcifer.py <port> <where a redirect goes> <log> <seconds to live>
With `none` for where, as the pre-flight has it with no door in front, nothing of it would move a browser off the page or wipe
what the page did, so the harness can see the script run."""
import http.server
import json
import sys
import threading

PORT, ELSEWHERE, LOG, LIVES = int(sys.argv[1]), sys.argv[2], sys.argv[3], float(sys.argv[4])
SCRIPT = b"document.documentElement.setAttribute('data-ran','yes');document.cookie='planted=1;path=/';fetch('/v1/machines');"
PAGE = b"<!doctype html><title>hostile</title><script>" + SCRIPT + b"</script><body>hostile</body>"
DRAWING = b'<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><script>' + SCRIPT + b"</script></svg>"
PLANTS = [
    ("Set-Cookie", "planted=1; Path=/"),
    ("Content-Security-Policy", "script-src * 'unsafe-inline'"),
    ("Content-Disposition", "inline"),
    ("Access-Control-Allow-Origin", "*"),
    ("Cozy-Said", "kept"),
]
if ELSEWHERE != "none":
    PLANTS += [
        ("Clear-Site-Data", '"*"'),
        ("Link", "</evil.js>; rel=preload; as=script"),
        ("Refresh", "0; url=" + ELSEWHERE),
        ("WWW-Authenticate", 'Basic realm="yours"'),
    ]
ANSWERS = {
    "/v1/door/page": (200, [("Content-Type", "text/html; charset=utf-8")], PAGE),
    "/v1/door/listed": (200, [("Content-Type", "application/json, text/html")], PAGE),
    "/v1/door/two": (200, [("Content-Type", "application/json"), ("Content-Type", "text/html")], PAGE),
    "/v1/door/json": (200, [("Content-Type", "application/json")], PAGE),
    "/v1/door/video": (200, [("Content-Type", "video/mp4")], PAGE),
    "/v1/door/svg": (200, [("Content-Type", "image/svg+xml")], DRAWING),
    "/v1/door/none": (200, [], PAGE),
    "/v1/door/module": (200, [("Content-Type", "text/javascript")], SCRIPT),
    "/v1/door/redirect": (302, [("Location", ELSEWHERE)], b""),
}


class Hostile(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def answer(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        with open(LOG, "a") as log:
            line = {"method": self.command, "path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}}
            log.write(json.dumps(line) + "\n")
        status, said, body = ANSWERS.get(self.path.split("?")[0], (200, [("Content-Type", "application/json")], b"{}"))
        self.send_response(status)
        for name, value in said + PLANTS:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = answer

    def log_message(self, *_: object) -> None:
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Hostile)
threading.Timer(LIVES, server.shutdown).start()
server.serve_forever()
