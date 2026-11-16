"""The cli suite's weightless requests: from cozy through TensorD, the Runtime and TensorFS to the executor's
reply and back, with no model, so they run on any computer."""

import ctypes
import socket
import struct
import threading
import time

import msgspec

from cozy_runtime.author import App, Telemetry
from cozy_runtime.media import File, Files

app = App()


class Text(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Reversed(msgspec.Struct):
    text: str


@app.entrypoint(accelerator=False)
def reverse(payload: Text) -> Reversed:
    return Reversed(text=payload.text[::-1])


class Wait(msgspec.Struct, forbid_unknown_fields=True):
    seconds: float


class Waited(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False)
def wait(payload: Wait) -> Waited:
    """A run that stays in progress: followed, snapshotted and canceled by the suite."""
    time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)


@app.entrypoint(accelerator=False)
def silent(payload: Wait, tel: Telemetry) -> Waited:
    """Says it waits, then sleeps without a word: nothing judges a run's progress (cl-663)."""
    with tel.stage("waiting silently"):
        time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)


class Nothing(msgspec.Struct, forbid_unknown_fields=True):
    pass


@app.entrypoint(accelerator=False)
def deadlock(payload: Nothing, tel: Telemetry) -> Nothing:
    """Takes a lock it holds, inside a stage: a hang, which TensorD ends once nothing of it moves (cl-977)."""
    held = threading.Lock()
    with tel.stage("deadlocked"):
        held.acquire()
        held.acquire()
    return Nothing()


@app.entrypoint(accelerator=False)
def spin(payload: Wait, tel: Telemetry) -> Waited:
    """Busy on its CPU without a word: a call that works, however long, which nothing ends (cl-977)."""
    with tel.stage("working silently"):
        end = time.monotonic() + payload.seconds
        while time.monotonic() < end:
            pass
    return Waited(seconds=payload.seconds)


class Peer(msgspec.Struct, forbid_unknown_fields=True):
    host: str
    port: int


class Answered(msgspec.Struct):
    reply: str


@app.entrypoint(accelerator=False)
def remote(payload: Peer, tel: Telemetry) -> Answered:
    """Asks a TCP peer and waits for its answer, with no timeout and no word: a slow peer that is there is no hang (cl-977).
    Dialled below Python, as a native library dials: the executor's sandbox refuses Python's own connect."""
    with tel.stage("waiting on a peer"), socket.socket() as peer:
        address = struct.pack("=H", socket.AF_INET) + struct.pack("!H", payload.port) + socket.inet_aton(payload.host) + bytes(8)
        if ctypes.CDLL(None, use_errno=True).connect(peer.fileno(), address, len(address)) != 0:
            raise OSError(ctypes.get_errno(), f"connect {payload.host}:{payload.port}")
        peer.sendall(b"ask\n")
        return Answered(reply=peer.makefile().readline().strip())


class Seen(msgspec.Struct):
    assets: list[str]


@app.entrypoint(accelerator=False)
def fidelities(payload: Nothing, assets: Files[File]) -> Seen:
    """Each attached asset as the executor was granted it: `label=fidelity`."""
    return Seen(assets=[f"{assets.info(i).label}={assets.info(i).fidelity}" for i in range(len(assets))])
