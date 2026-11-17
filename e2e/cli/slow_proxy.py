"""state_restart (cl-1010): a forward proxy that answers every request only after DELAY seconds, as a laptop's network to PyPI and the Hub
does. Calcifer is started with HTTP_PROXY/HTTPS_PROXY naming it; its machine link takes no proxy.

  slow_proxy.py <port file> <delay seconds>   listens on a free loopback port, written to the port file, until killed
"""
import http.client, select, socket, socketserver, sys, threading, time
from urllib.parse import urlsplit
from http.server import BaseHTTPRequestHandler


class Slow(BaseHTTPRequestHandler):
    delay = 3.0
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        sys.stderr.write(f"{time.strftime('%H:%M:%S')} {self.command} {self.path}\n")

    def do_CONNECT(self) -> None:
        time.sleep(self.delay)
        host, _, port = self.path.rpartition(":")
        try:
            upstream = socket.create_connection((host, int(port)), timeout=30)
        except OSError:
            self.send_error(502)
            return
        self.send_response(200, "Connection established")
        self.end_headers()
        sockets = [self.connection, upstream]
        while True:
            ready, _, _ = select.select(sockets, [], [], 60)
            if not ready:
                break
            for sock in ready:
                data = sock.recv(65536)
                if not data:
                    upstream.close()
                    return
                (upstream if sock is self.connection else self.connection).sendall(data)
        upstream.close()

    def forward(self) -> None:
        time.sleep(self.delay)
        target = urlsplit(self.path)
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0)) if self.headers.get("Content-Length") else None
        headers = {name: value for name, value in self.headers.items() if name.lower() not in ("proxy-connection", "connection")}
        conn = http.client.HTTPConnection(target.hostname or "127.0.0.1", target.port or 80, timeout=120)
        conn.request(self.command, target.path + (f"?{target.query}" if target.query else ""), body=body, headers=headers)
        answer = conn.getresponse()
        self.send_response(answer.status, answer.reason)
        for name, value in answer.getheaders():
            if name.lower() not in ("transfer-encoding", "connection"):
                self.send_header(name, value)
        data = answer.read()
        if answer.getheader("Content-Length") is None:
            self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
        conn.close()

    do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PATCH = forward


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main() -> None:
    Slow.delay = float(sys.argv[2])
    with Server(("127.0.0.1", 0), Slow) as server:
        with open(sys.argv[1], "w") as file:
            file.write(str(server.server_address[1]))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        while True:
            time.sleep(3600)


if __name__ == "__main__":
    main()
