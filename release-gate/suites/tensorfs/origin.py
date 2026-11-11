"""An origin of a store's blobs for the burst case: /blobs/<path> answers 307 to /storage/<path> (max-age=300, as the
Hub does), which serves the blob with Range through one link of LINK bytes/s. From FROM to TO s after the first storage
ask, every storage ask is answered 503, as R2 did 13 times near the end of run 32321's download.
    origin.py <port> <store> <log> <link bytes/s> <from s> <to s>"""
import json
import os
import sys
import threading
import time
from collections.abc import Iterable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, STORE, LOG, LINK, FROM, TO = int(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4]), float(sys.argv[5]), float(sys.argv[6])
lock = threading.Lock()
link = {'at': time.time(), 'tokens': 0.0}
began: list[float] = []


def log(**row: str | int | bool) -> None:
    with lock, open(LOG, 'a') as f:
        f.write(json.dumps({'at': time.time(), **row}) + '\n')


def through(n: int) -> None:
    """Wait until the link has carried n more bytes: one bucket for every request."""
    while True:
        with lock:
            now = time.time()
            link['tokens'] = min(max(LINK / 10, n), link['tokens'] + (now - link['at']) * LINK)
            link['at'] = now
            if link['tokens'] >= n:
                link['tokens'] -= n
                return
            wait = (n - link['tokens']) / LINK
        time.sleep(wait)


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, format: str, *args: object) -> None:
        pass

    def short(self, code: int, headers: Iterable[tuple[str, str]] = ()) -> None:
        self.send_response(code)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def do_HEAD(self) -> None:
        rest = self.path.split('?')[0].lstrip('/').partition('/')[2]
        blob = os.path.join(STORE, 'blobs', rest)
        if not os.path.isfile(blob):
            return self.short(404)
        self.send_response(200)
        self.send_header('Content-Length', str(os.path.getsize(blob)))
        self.end_headers()

    def do_GET(self) -> None:
        path = self.path.split('?')[0]
        kind, _, rest = path.lstrip('/').partition('/')
        blob = os.path.join(STORE, 'blobs', rest)
        if kind not in ('blobs', 'storage') or not os.path.isfile(blob):
            return self.short(404)
        if kind == 'blobs':
            return self.short(307, (('Location', f'/storage/{rest}'), ('Cache-Control', 'max-age=300')))
        with lock:
            began.append(time.time())
            busy = FROM <= time.time() - began[0] < TO
        if busy:
            log(kind='object', path=path, status=503, bytes=0, slow=False, ask=1)
            return self.short(503)
        size = os.path.getsize(blob)
        first, last, status = 0, size - 1, 200
        if self.headers.get('range'):
            a, b = self.headers['range'].split('=', 1)[1].split('-')
            first, last, status = int(a), min(int(b) if b else size - 1, size - 1), 206
        self.send_response(status)
        if status == 206:
            self.send_header('Content-Range', f'bytes {first}-{last}/{size}')
        self.send_header('Content-Length', str(last - first + 1))
        self.end_headers()
        sent = 0
        with open(blob, 'rb') as f:
            f.seek(first)
            try:
                while sent < last - first + 1:
                    data = f.read(min(1 << 16, last - first + 1 - sent))
                    through(len(data))
                    self.wfile.write(data)
                    sent += len(data)
            except OSError:  # the client left
                pass
        log(kind='object', path=path, status=status, bytes=sent, slow=False, ask=1)


ThreadingHTTPServer.daemon_threads = True
ThreadingHTTPServer.request_queue_size = 1024
ThreadingHTTPServer(('127.0.0.1', PORT), Handler).serve_forever()
