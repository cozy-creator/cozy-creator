"""e2e: forwards TCP from LISTEN_HOST:LISTEN_PORT to TARGET_HOST:TARGET_PORT.

The isolated Hub listens on this computer's loopback, where a rented docker container can't reach it. One of these runs
inside the container on its loopback, at the Hub's port, and one on Docker's bridge, to the Hub: the pod dials the Hub's
own origin and its certificate holds. Killing the bridge one keeps the pod from its Hub.

    python3 forward.py LISTEN_HOST LISTEN_PORT TARGET_HOST TARGET_PORT
"""

import asyncio
import sys


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except OSError:
        pass
    finally:
        writer.close()


async def serve(listen_host: str, listen_port: int, target_host: str, target_port: int) -> None:
    async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            upstream_reader, upstream_writer = await asyncio.open_connection(target_host, target_port)
        except OSError:
            writer.close()
            return
        await asyncio.gather(pipe(reader, upstream_writer), pipe(upstream_reader, writer))

    server = await asyncio.start_server(handle, listen_host, listen_port)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(serve(sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])))
