"""A Hub in front of the Hub, for the straggler case: every request goes on to the real Hub as sent; an object
the Hub redirects to storage is fetched here instead, and the first STRAGGLERS of them are sent at RATE bytes/s
the first time they are asked for, as a slow storage GET is (release-gates-no-regressions, 10-01). A second ask for
the same object is answered at full speed: TensorFS must re-ask a straggler rather than wait out its tail.
With BURST, the first STRAGGLERS objects of at least 4 BURST bytes are cold, as a cold R2 object is (10-01, tfs-138):
every ask of one sends BURST bytes at full speed, then drips at RATE until the client gives it up. TensorFS must keep
re-asking such an object, however often it was re-asked before, rather than wait out a drip.
With MIN too, BURST is one 64 KiB and only objects of at least MIN bytes are cold, as R2's coldest are (tfs-141): each ask
moves little however often it is asked, and TensorFS must read the object as parallel ranges.
With FAST too, a cold object's asks of a part (TensorFS's 16 MiB range) or less come at full speed: many parts write at
once, and writing them must not stand in the way of joining them up (a whole download hung so, tfs-143).
With --link, every object byte goes through one link of that many bytes/s, as a slow pod's: TensorFS must keep moving
however slow each request gets (run 31103: 72 objects judged slow at 5 MB/s, then nothing for good).
With --silent, the first ask of 256 KiB to 1 MiB gets its headers and half its body, then a socket held open and silent:
a range too small to split, the last left, which only the no-bytes stall can end (run 32321 sat 16 min on its last two).
    straggler.py <port> <upstream> <stragglers> <rate> <log> [<burst> [<min> [fast]]] [--link <bytes/s>] [--silent]"""
import argparse
import http.client
import json
import socket
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import cast


class Args(argparse.Namespace):
    port: str
    upstream: str
    stragglers: str
    rate: str
    log: str
    burst: str | None
    min: str | None
    fast: str | None
    link: int
    silent: bool


ap = argparse.ArgumentParser()
for name in ('port', 'upstream', 'stragglers', 'rate', 'log'):
    ap.add_argument(name)
for name in ('burst', 'min', 'fast'):
    ap.add_argument(name, nargs='?')
ap.add_argument('--link', type=int, default=0)
ap.add_argument('--silent', action='store_true')
a = ap.parse_args(namespace=Args())
PORT, UPSTREAM, STRAGGLERS, RATE, LOG = int(a.port), a.upstream, int(a.stragglers), int(a.rate), a.log
BURST = int(a.burst or 0)
MIN = int(a.min) if a.min else 4 * BURST
FAST, LINK, SILENT = bool(a.fast), a.link, a.silent
PART = 16 << 20
up = urllib.parse.urlsplit(UPSTREAM)
lock = threading.Lock()
slowed: set[str] = set()
asked: dict[str, int] = {}
silenced: set[str] = set()
HOP = {'connection', 'keep-alive', 'transfer-encoding', 'te', 'trailer', 'upgrade', 'proxy-authorization', 'host'}


def log(**row: str | float | bool | None) -> None:
    with lock, open(LOG, 'a') as f:
        f.write(json.dumps({'at': time.time(), **row}) + '\n')


link = {'at': time.time(), 'tokens': 0.0}


def through(n: int) -> None:
    """Wait until the link (--link) has carried n more bytes: one bucket for every request."""
    while LINK:
        with lock:
            now = time.time()
            link['tokens'] = min(max(LINK / 10, n), link['tokens'] + (now - link['at']) * LINK)
            link['at'] = now
            if link['tokens'] >= n:
                link['tokens'] -= n
                return
            wait = (n - link['tokens']) / LINK
        time.sleep(wait)


pool = threading.local()


def fetch(url: urllib.parse.SplitResult, method: str, target: str, body: bytes | None,  # type: ignore[return]  # the second attempt returns or re-raises
          headers: dict[str, str]) -> http.client.HTTPResponse:
    """One request upstream over this thread's kept-alive connection to that host, made again once if it dropped."""
    conns: dict[str, http.client.HTTPConnection] = pool.__dict__.setdefault('conns', {})
    for attempt in (0, 1):
        c = conns.get(url.netloc)
        if c is None:
            c = conns[url.netloc] = (http.client.HTTPSConnection if url.scheme == 'https' else http.client.HTTPConnection)(
                url.netloc, timeout=120)
        try:
            c.request(method, target, body=body, headers={**headers, 'Host': url.netloc})
            return c.getresponse()
        except (OSError, http.client.HTTPException):
            c.close()
            conns.pop(url.netloc, None)
            if attempt:
                raise


def forget(url: urllib.parse.SplitResult) -> None:
    c: http.client.HTTPConnection | None = pool.__dict__.get('conns', {}).pop(url.netloc, None)
    if c is not None:
        c.close()


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    connection: socket.socket

    def log_message(self, format: str, *args: object) -> None:
        pass

    def send(self, data: bytes) -> None:
        through(len(data))
        self.wfile.write(data)

    def hold(self, resp: http.client.HTTPResponse) -> int:
        """Headers and half the body, then silence until the client gives the socket up (at most 5 min)."""
        self.send_response(resp.status)
        for k, v in resp.getheaders():
            if k.lower() not in HOP:
                self.send_header(k, v)
        self.end_headers()
        length = resp.getheader('content-length')
        assert length is not None
        half = resp.read(int(length) // 2)
        self.send(half)
        self.wfile.flush()
        self.connection.settimeout(1)
        end = time.time() + 300
        while time.time() < end:
            try:
                if self.connection.recv(1, socket.MSG_PEEK) == b'':
                    break
            except socket.timeout:
                continue
            except OSError:
                break
        self.close_connection = True
        return len(half)

    def relay(self, resp: http.client.HTTPResponse, slow: bool = False) -> int:
        self.send_response(resp.status)
        for k, v in resp.getheaders():
            if k.lower() not in HOP and k.lower() != 'content-length':
                self.send_header(k, v)
        if not slow:  # streamed as it comes, as storage answers
            length = resp.getheader('content-length')
            bodyless = self.command == 'HEAD' or resp.status in (204, 304)
            if length:
                self.send_header('Content-Length', length)
            elif not bodyless:
                self.send_header('Transfer-Encoding', 'chunked')
            self.end_headers()
            if bodyless:
                return 0
            sent = 0
            try:
                while chunk := resp.read1(1 << 16):
                    self.send(chunk if length else b'%x\r\n%s\r\n' % (len(chunk), chunk))
                    sent += len(chunk)
                if not length:
                    self.send(b'0\r\n\r\n')
            except OSError:
                pass
            return sent
        if BURST:  # a cold object: BURST bytes as they come, then the rest dripped
            self.send_header('Content-Length', cast(str, resp.getheader('content-length')))  # cast: a storage object states its length
            self.end_headers()
            sent = 0
            try:
                while sent < BURST and (chunk := resp.read1(min(1 << 16, BURST - sent))):
                    self.send(chunk)
                    sent += len(chunk)
                self.wfile.flush()
                rest = resp.read()
                for i in range(0, len(rest), RATE // 4 or 1):
                    time.sleep(0.25)
                    self.send(rest[i:i + (RATE // 4 or 1)])
                    self.wfile.flush()
                    sent += len(rest[i:i + (RATE // 4 or 1)])
            except OSError:  # the client gave up on it: what a cold object's drip should cause
                pass
            return sent
        if slow:
            length = resp.getheader('content-length')
            if length:
                self.send_header('Content-Length', length)
            else:
                self.send_header('Transfer-Encoding', 'chunked')
            self.end_headers()
            sent = 0
            while chunk := resp.read(RATE // 4 or 1):
                try:
                    if length:
                        self.send(chunk)
                    else:
                        self.send(b'%x\r\n%s\r\n' % (len(chunk), chunk))
                    self.wfile.flush()
                except OSError:  # the client gave up on it: what a straggler should cause
                    return sent
                sent += len(chunk)
                time.sleep(0.25)
            if not length:
                self.send(b'0\r\n\r\n')
            return sent

    def handle_any(self) -> None:
        n = int(self.headers.get('content-length') or 0)
        data = self.rfile.read(n) if n else None
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        resp = fetch(up, self.command, self.path, data, headers)
        where = resp.getheader('location')
        if resp.status in (301, 302, 303, 307, 308) and where and self.command == 'GET':
            resp.read()
            loc = urllib.parse.urlsplit(where)
            fwd = {k: v for k, v in headers.items() if k.lower() in ('range', 'accept', 'accept-encoding', 'if-none-match')}
            t0 = time.time()
            got = fetch(loc, 'GET', loc.path + (f'?{loc.query}' if loc.query else ''), None, fwd)
            length = int(got.getheader('content-length') or 0)
            # The object's size: a range's Content-Range total, else the answer's length.
            size = int((got.getheader('content-range') or '/').rsplit('/', 1)[1] or length) if got.status == 206 else length
            with lock:
                asked[self.path] = asked.get(self.path, 0) + 1
                if BURST:
                    slow = self.path in slowed or (asked[self.path] == 1 and len(slowed) < STRAGGLERS and size >= MIN)
                else:
                    slow = self.path not in slowed and len(slowed) < STRAGGLERS
                if slow:
                    slowed.add(self.path)
            if FAST and slow and got.status in (200, 206) and length <= PART:
                slow = False
            with lock:
                silent = SILENT and not silenced and got.status in (200, 206) and 256 << 10 <= length <= 1 << 20
                if silent:
                    silenced.add(self.path)
            sent = self.hold(got) if silent else self.relay(got, slow=slow)
            if not got.isclosed():  # the client left mid-body: that connection's answer is unread
                forget(loc)
            # `slow`: the object is a straggler (cold); `dripped`: this ask was sent slowly.
            log(kind='object', path=self.path, slow=self.path in slowed, dripped=slow, ask=asked[self.path], range=fwd.get('Range') or fwd.get('range'),
                length=length, bytes=sent, s=round(time.time() - t0, 2), silent=silent, silenced=self.path in silenced)
            return
        sent = self.relay(resp)
        if not resp.isclosed():
            forget(up)
        log(kind='api', method=self.command, path=self.path.split('?')[0], status=resp.status, bytes=sent)

    do_GET = do_POST = do_PUT = do_HEAD = do_DELETE = do_PATCH = handle_any


# A client keeps 128 requests in flight: the default backlog of 5 drops their connects (seconds of SYN backoff).
ThreadingHTTPServer.request_queue_size = 1024
ThreadingHTTPServer(('127.0.0.1', PORT), Handler).serve_forever()
