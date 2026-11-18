"""Slow links for e2e/cli/rcmoves.sh (cl-1201), as Paul's laptop has them: GitHub at about 2 MB/s (the RC's 99 MB cozy took 49 s) and
an uplink to a rental at about 2 MB/s (sending it took 37 s).

  slow.py relay <host:port> <bytes/s>      a TCP relay to a machine: what each connection sends it, at most <bytes/s>
  slow.py connect <bytes/s> <suffix>...    an HTTP CONNECT proxy: what a host ending in a suffix answers, at most <bytes/s> a connection;
                                           any other host as fast as it goes

Each listens on a free loopback port, writes it to stdout, and serves until killed."""

import asyncio
import sys
import time

CHUNK = 16 * 1024


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter, rate: float) -> None:
    began, sent = time.monotonic(), 0
    try:
        while data := await reader.read(CHUNK):
            if rate > 0:
                sent += len(data)
                ahead = sent / rate - (time.monotonic() - began)
                if ahead > 0:
                    await asyncio.sleep(ahead)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def listening(joined) -> None:
    server = await asyncio.start_server(joined, "127.0.0.1", 0)
    print(server.sockets[0].getsockname()[1], flush=True)
    async with server:
        await server.serve_forever()


async def relay(target: str, rate: float) -> None:
    host, _, to = target.rpartition(":")

    async def joined(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            up_reader, up_writer = await asyncio.open_connection(host, int(to))
        except OSError:
            writer.close()
            return
        await asyncio.gather(pipe(reader, up_writer, rate), pipe(up_reader, writer, 0))

    await listening(joined)


async def connect(rate: float, slow: list[str]) -> None:
    async def joined(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            head = await reader.readuntil(b"\r\n\r\n")
            method, target, _ = head.split(b"\r\n", 1)[0].decode().split(" ", 2)
            if method != "CONNECT":
                writer.write(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                await writer.drain()
                writer.close()
                return
            host, _, to = target.rpartition(":")
            up_reader, up_writer = await asyncio.open_connection(host, int(to))
        except (OSError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
            writer.close()
            return
        writer.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        await writer.drain()
        down = rate if any(host == s or host.endswith("." + s) for s in slow) else 0
        await asyncio.gather(pipe(reader, up_writer, 0), pipe(up_reader, writer, down))

    await listening(joined)


def main() -> None:
    match sys.argv[1:]:
        case ["relay", target, rate]:
            asyncio.run(relay(target, float(rate)))
        case ["connect", rate, *slow]:
            asyncio.run(connect(float(rate), slow))
        case _:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
