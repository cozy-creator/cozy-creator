"""A Hub in front of the Hub, for the straggler case: every request goes on to the real Hub as sent; an object
the Hub redirects to storage is fetched here instead, and the first STRAGGLERS of them are sent at RATE bytes/s
the first time they are asked for, as a slow storage GET is (release-gates-no-regressions, 10-01). A second ask for
the same object is answered at full speed: TensorFS must re-ask a straggler rather than wait out its tail.
With BURST, the first STRAGGLERS objects of at least 4 BURST bytes are cold, as a cold R2 object is (10-01, tfs-138):
every ask of one sends BURST bytes at full speed, then drips at RATE until the client gives it up. TensorFS must keep
re-asking such an object, however often it was re-asked before, rather than wait out a drip.
    straggler.py <port> <upstream> <stragglers> <rate> <log> [<burst>]"""
import http.client
import json
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, UPSTREAM, STRAGGLERS, RATE, LOG = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
BURST = int(sys.argv[6]) if len(sys.argv) > 6 else 0
up = urllib.parse.urlsplit(UPSTREAM)
lock = threading.Lock()
slowed, asked = set(), {}
HOP = {'connection', 'keep-alive', 'transfer-encoding', 'te', 'trailer', 'upgrade', 'proxy-authorization', 'host'}


def log(**row):
    with lock, open(LOG, 'a') as f:
        f.write(json.dumps({'at': time.time(), **row}) + '\n')


pool = threading.local()


def fetch(url, method, target, body, headers):
    """One request upstream over this thread's kept-alive connection to that host, made again once if it dropped."""
    conns = pool.__dict__.setdefault('conns', {})
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


def forget(url):
    c = pool.__dict__.get('conns', {}).pop(url.netloc, None)
    if c is not None:
        c.close()


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_):
        pass

    def relay(self, resp, slow=False):
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
                    self.wfile.write(chunk if length else b'%x\r\n%s\r\n' % (len(chunk), chunk))
                    sent += len(chunk)
                if not length:
                    self.wfile.write(b'0\r\n\r\n')
            except OSError:
                pass
            return sent
        if BURST:  # a cold object: BURST bytes as they come, then the rest dripped
            self.send_header('Content-Length', resp.getheader('content-length'))
            self.end_headers()
            sent = 0
            try:
                while sent < BURST and (chunk := resp.read1(min(1 << 16, BURST - sent))):
                    self.wfile.write(chunk)
                    sent += len(chunk)
                self.wfile.flush()
                rest = resp.read()
                for i in range(0, len(rest), RATE // 4 or 1):
                    time.sleep(0.25)
                    self.wfile.write(rest[i:i + (RATE // 4 or 1)])
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
                        self.wfile.write(chunk)
                    else:
                        self.wfile.write(b'%x\r\n%s\r\n' % (len(chunk), chunk))
                    self.wfile.flush()
                except OSError:  # the client gave up on it: what a straggler should cause
                    return sent
                sent += len(chunk)
                time.sleep(0.25)
            if not length:
                self.wfile.write(b'0\r\n\r\n')
            return sent

    def handle_any(self):
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
            with lock:
                asked[self.path] = asked.get(self.path, 0) + 1
                if BURST:
                    slow = self.path in slowed or (asked[self.path] == 1 and len(slowed) < STRAGGLERS and length >= 4 * BURST)
                else:
                    slow = self.path not in slowed and len(slowed) < STRAGGLERS
                if slow:
                    slowed.add(self.path)
            sent = self.relay(got, slow=slow)
            if not got.isclosed():  # the client left mid-body: that connection's answer is unread
                forget(loc)
            log(kind='object', path=self.path, slow=slow, ask=asked[self.path], range=fwd.get('Range') or fwd.get('range'),
                length=length, bytes=sent, s=round(time.time() - t0, 2))
            return
        sent = self.relay(resp)
        if not resp.isclosed():
            forget(up)
        log(kind='api', method=self.command, path=self.path.split('?')[0], status=resp.status, bytes=sent)

    do_GET = do_POST = do_PUT = do_HEAD = do_DELETE = do_PATCH = handle_any


ThreadingHTTPServer(('127.0.0.1', PORT), Handler).serve_forever()
