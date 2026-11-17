#!/usr/bin/env python3
"""phantom_docker.py LISTEN UPSTREAM LOG: a Docker Engine socket at LISTEN that is UPSTREAM's, except that every container
create asks a million CPUs, which the engine refuses (400 "range of CPUs is from 0.01 to N.00"): a provider that lists stock
and refuses to sell it, as RunPod did with RTX 5090s (th-383). Each create is a line of LOG. One request per connection."""
import asyncio
import json
import re
import sys
import time

LISTEN, UPSTREAM, LOG = sys.argv[1:4]
CREATE = re.compile(rb"^POST /v[0-9.]+/containers/create[?\s]")


async def serve(client_r: asyncio.StreamReader, client_w: asyncio.StreamWriter) -> None:
    try:
        head = await client_r.readuntil(b"\r\n\r\n")
        lines = head[:-4].split(b"\r\n")
        length = 0
        kept = [lines[0]]
        for line in lines[1:]:
            name = line.split(b":", 1)[0].strip().lower()
            if name == b"content-length":
                length = int(line.split(b":", 1)[1])
            elif name != b"connection":
                kept.append(line)
        body = await client_r.readexactly(length) if length else b""
        if CREATE.match(lines[0]) and body:
            create = json.loads(body)
            create.setdefault("HostConfig", {})["NanoCpus"] = 10**15
            body = json.dumps(create).encode()
            with open(LOG, "a") as log:
                log.write(f"{time.time():.3f} {lines[0].decode(errors='replace')}\n")
        kept += [b"Content-Length: %d" % len(body)] if length or body else []
        kept.append(b"Connection: close")
        up_r, up_w = await asyncio.open_unix_connection(UPSTREAM)
        up_w.write(b"\r\n".join(kept) + b"\r\n\r\n" + body)
        await up_w.drain()
        while chunk := await up_r.read(65536):
            client_w.write(chunk)
            await client_w.drain()
        up_w.close()
    except (asyncio.IncompleteReadError, ConnectionError):
        pass
    finally:
        client_w.close()


async def main() -> None:
    server = await asyncio.start_unix_server(serve, path=LISTEN)
    async with server:
        await server.serve_forever()


asyncio.run(main())
