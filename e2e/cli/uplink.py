"""hold_wins (cl-1196): a laptop's uplink to its rental. A TCP relay from a free loopback port to 127.0.0.1:<to>: what its clients send
goes on at <bytes per second> in all, shared, and what comes back at full speed. Calcifer reaches the stood-in rental through it.

  uplink.py <port file> <to port> <bytes per second>   until killed
"""
import asyncio
import os
import sys
import time


class Uplink:
    def __init__(self, rate: float) -> None:
        self.rate, self.free = rate, time.monotonic()

    async def take(self, n: int) -> None:
        now = time.monotonic()
        self.free = max(self.free, now) + n / self.rate
        await asyncio.sleep(self.free - now)


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter, uplink: Uplink | None) -> None:
    try:
        while data := await reader.read(16384):
            if uplink:
                await uplink.take(len(data))
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        writer.close()


async def main(port_file: str, to: int, rate: float) -> None:
    uplink = Uplink(rate)

    async def relay(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            far_reader, far_writer = await asyncio.open_connection("127.0.0.1", to)
        except OSError:
            writer.close()
            return
        await asyncio.gather(pipe(reader, far_writer, uplink), pipe(far_reader, writer, None))

    server = await asyncio.start_server(relay, "127.0.0.1", 0)
    with open(port_file + ".tmp", "w") as out:
        out.write(f"{server.sockets[0].getsockname()[1]}\n")
    os.replace(port_file + ".tmp", port_file)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], int(sys.argv[2]), float(sys.argv[3])))
