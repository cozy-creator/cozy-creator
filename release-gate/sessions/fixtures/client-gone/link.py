"""The line between a computer and a machine, on one pod: forwards `listen` to `target`, the machine's bytes at the rate its control
file says. `up`: forward, as fast as it can. `up <bytes/s>`: forward at that rate. `down`: nothing listens, and what was open is cut,
as when a laptop loses its network."""
import asyncio
import sys
import time

listen, host, port, control = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4]
writers = set()
free_at = 0.0


def said():
    try:
        words = open(control).read().split()
    except OSError:
        words = ["up"]
    return (words[0] if words else "up"), (int(words[1]) if len(words) > 1 else 0)


async def carry(reader, writer, paced):
    global free_at
    try:
        while data := await reader.read(1 << 16):
            rate = said()[1] if paced else 0
            if rate:
                # One line, whatever crosses it.
                now = time.monotonic()
                free_at = max(free_at, now) + len(data) / rate
                await asyncio.sleep(free_at - now)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError, asyncio.CancelledError):
        pass
    finally:
        writer.close()


async def serve(client_reader, client_writer):
    try:
        machine_reader, machine_writer = await asyncio.open_connection(host, port)
    except OSError:
        client_writer.close()
        return
    writers.update((client_writer, machine_writer))
    await asyncio.gather(carry(client_reader, machine_writer, False), carry(machine_reader, client_writer, True), return_exceptions=True)
    writers.difference_update((client_writer, machine_writer))


async def main():
    server = None
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
