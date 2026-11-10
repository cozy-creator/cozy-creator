"""A package index as PyPI answers one, Cache-Control and all (max-age=600), over a folder of wheels.

    index.py wheel DIR NAME VERSION    an empty wheel of NAME at VERSION, in DIR
    index.py serve DIR                 serves DIR on a free port, said as host:port on stdout; ends after 10 idle minutes
"""
import hashlib
import http.server
import os
import sys
import threading
import time
import zipfile


def wheel(folder: str, name: str, version: str) -> None:
    dist = f"{name}-{version}.dist-info"
    files = {
        f"{name}/__init__.py": f"VERSION = {version!r}\n",
        f"{dist}/METADATA": f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n",
        f"{dist}/WHEEL": "Wheel-Version: 1.0\nGenerator: e2e\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    }
    record = "".join(f"{path},,\n" for path in files) + f"{dist}/RECORD,,\n"
    with zipfile.ZipFile(os.path.join(folder, f"{name}-{version}-py3-none-any.whl"), "w") as whl:
        for path, text in {**files, f"{dist}/RECORD": record}.items():
            whl.writestr(path, text)


def serve(folder: str) -> None:
    last = [time.time()]

    class Index(http.server.BaseHTTPRequestHandler):
        def do_HEAD(self) -> None:
            self.answer(False)

        def do_GET(self) -> None:
            self.answer(True)

        def answer(self, body_too: bool) -> None:
            last[0] = time.time()
            parts = self.path.strip("/").split("/")
            if parts[0] == "files" and len(parts) == 2 and os.path.isfile(os.path.join(folder, parts[1])):
                body, kind = open(os.path.join(folder, parts[1]), "rb").read(), "application/octet-stream"
            elif parts[0] == "simple" and len(parts) == 2:
                links = "".join(
                    f'<a href="/files/{f}#sha256={hashlib.sha256(open(os.path.join(folder, f), "rb").read()).hexdigest()}">{f}</a>'
                    for f in sorted(os.listdir(folder)) if f.split("-")[0] == parts[1]
                )
                body, kind = f"<!DOCTYPE html><html><body>{links}</body></html>".encode(), "text/html"
            else:
                return self.send_error(404)
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "max-age=600, public")
            self.end_headers()
            if body_too:
                self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            sys.stderr.write(f"{self.address_string()} {format % args}\n")

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Index)
    print(f"127.0.0.1:{server.server_address[1]}", flush=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    while time.time() - last[0] < 600:
        time.sleep(5)


if __name__ == "__main__":
    {"wheel": lambda: wheel(*sys.argv[2:5]), "serve": lambda: serve(sys.argv[2])}[sys.argv[1]]()
