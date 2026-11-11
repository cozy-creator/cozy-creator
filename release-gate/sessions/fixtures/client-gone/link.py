"""The line between a computer and a machine, on one pod: forwards `listen` to `target`, the machine's bytes at the rate its control
file says. `up`: forward, as fast as it can. `up <bytes/s>`: forward at that rate. `up <bytes/s> both`: the computer's bytes too, at
that rate in each direction. `down`: nothing listens, and what was open is cut, as when a laptop loses its network."""
import asyncio
import sys
import time

listen, host, port, control = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4]
writers: set[asyncio.StreamWriter] = set()
free_at = {True: 0.0, False: 0.0}


def said() -> tuple[str, int, bool]:
    try:
        words = open(control).read().split()
    except OSError:
        words = ["up"]
    return (words[0] if words else "up"), (int(words[1]) if len(words) > 1 else 0), words[2:] == ["both"]


async def carry(reader: asyncio.StreamReader, writer: asyncio.StreamWriter, paced: bool) -> None:
    global free_at
    try:
        while data := await reader.read(1 << 16):
            _, rate, both = said()
            rate = rate if paced or both else 0
            if rate:
                # One line each way, whatever crosses it.
                now = time.monotonic()
                free_at[paced] = max(free_at[paced], now) + len(data) / rate
                await asyncio.sleep(free_at[paced] - now)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError, asyncio.CancelledError):
        pass
    finally:
        writer.close()


async def serve(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    try:
        machine_reader, machine_writer = await asyncio.open_connection(host, port)
    except OSError:
        client_writer.close()
        return
    writers.update((client_writer, machine_writer))
    await asyncio.gather(carry(client_reader, machine_writer, False), carry(machine_reader, client_writer, True), return_exceptions=True)
    writers.difference_update((client_writer, machine_writer))


async def main() -> None:
    server: asyncio.Server | None = None
    while True:
        state = said()[0]
        if state == "down" and server:
            server.close()
            server = None
            for writer in list(writers):
                writer.transport.abort()
            writers.clear()
        elif state != "down" and not server:
            server = await asyncio.start_server(serve, "127.0.0.1", listen, reuse_address=True)
        await asyncio.sleep(0.2)


print("link", time.time(), flush=True)
asyncio.run(main())
