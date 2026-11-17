"""e2e/cli/egress.sh's `token_scope` (xs-202): which origin the token `tensorfs.configure` was given reaches.
Two origins on this computer answer every request 404 and keep its Authorization header; each call opens a model
neither has, and the headers its origin got go to stdout as JSON.

  token_scope.py configured    `configure` names origin A and the token
  token_scope.py environment   TENSORFS_ENDPOINT names origin A; `configure` gives the token alone
"""

import http.server
import json
import os
import sys
import tempfile
import threading

mode = sys.argv[1]


class Origin(http.server.ThreadingHTTPServer):
    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), Asked)
        self.got: list[str | None] = []
        self.url = f"http://127.0.0.1:{self.server_port}"
        threading.Thread(target=self.serve_forever, daemon=True).start()


class Asked(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        assert isinstance(self.server, Origin)
        self.server.got.append(self.headers.get("authorization"))
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    do_HEAD = do_POST = do_PUT = do_GET

    def log_message(self, *_: object) -> None:
        pass


a, b = Origin(), Origin()
# Read by TensorFS once, at its first fetch: loopback and plaintext http are fetched at declared origins only.
os.environ["TENSORFS_LOCAL_ORIGINS"] = f"{a.url},{b.url}"
os.environ.pop("TENSORFS_TOKEN", None)
if mode == "environment":
    os.environ["TENSORFS_ENDPOINT"] = a.url

import tensorfs  # noqa: E402

cache = tempfile.mkdtemp(prefix="token-scope-")
if mode == "configured":
    tensorfs.configure(endpoint=a.url, token="token-a", cache_dir=cache)
else:
    tensorfs.configure(token="token-a", cache_dir=cache)


def got(origin: Origin, endpoint: str | None = None, token: str | None = None) -> list[str | None]:
    """The Authorization header of each request `origin` got from one open."""
    before = len(origin.got)
    try:
        tensorfs.open("e2e/token-scope", endpoint=endpoint, token=token)
    except tensorfs.Error:
        pass
    return origin.got[before:]


print(json.dumps({
    "default": got(a),
    "named": got(a, a.url + "/"),
    "other": got(b, b.url),
    "other_own": got(b, b.url, "token-b"),
}))
