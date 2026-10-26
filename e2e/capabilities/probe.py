# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19", "tensorfs"]
# ///
"""What package code on a machine holds of its run's Hub access (cl-675), from inside its executor: the TensorFS
endpoint and token TensorD gave it, every JWT or private key anywhere in its heap or environment, and whether it can
read TensorD's memory. Its `weights` (a private commit: `model.weights=<org/name@sha256:…>`) make its run carry the
signer's capability. After `seconds` (past a token's life: it reads under one its machine renewed) it reads a tensor of
them, and opens `ref` (that commit) with TensorFS into an empty cache: through the endpoint it was given, and at the Hub
with the same token. Package code cannot dial on its own (the Runtime's no_network fence)."""

import gc
import os
import re
import tempfile
import time

import tensorfs

import msgspec

from cozy_runtime.author import App, Weights

app = App()

JWT = re.compile(r"eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{16,}")
PRIVATE = re.compile(r"PRIVATE KEY|\"d\"\s*:\s*\"[A-Za-z0-9_-]{40,}")


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    #: The Hub's origin, where the given token is tried directly.
    hub: str = "https://tensorhub.com"
    #: The weights' commit (org/name@sha256:<hex>), opened afresh.
    ref: str = ""
    seconds: float = 0


class Report(msgspec.Struct):
    endpoints: list[str]
    tokens: list[str]
    jwts: int
    private_keys: int
    secrets_in_environment: list[str]
    tensord_memory: str
    tensord_environment: str
    opened_through_endpoint: str
    opened_at_hub_with_token: str
    weights_read: str


def _scan() -> tuple[int, int]:
    """JWTs and private keys among the strings of plain dicts, lists and tuples, counted as it goes: a
    dict subclass (a lazy weights mapping) is never asked for its values, and nothing is kept."""
    jwts = keys = 0
    for obj in gc.get_objects():
        kind = type(obj)
        if kind is dict:
            items = (v for pair in obj.items() for v in pair)
        elif kind is list or kind is tuple:
            items = iter(obj)
        else:
            continue
        for item in items:
            if type(item) is str and len(item) < 1 << 16:
                jwts += "eyJ" in item and bool(JWT.search(item))
                keys += bool(PRIVATE.search(item))
    return jwts, keys


def _open(ref: str, endpoint: str, token: str) -> str:
    """`ref` opened into an empty cache and one tensor read: what fetched, or why not."""
    with tempfile.TemporaryDirectory() as cache:
        try:
            checkpoint = tensorfs.open(ref, cache_dir=cache, endpoint=endpoint, token=token)
            component = next(name for name, held in checkpoint.components.items() if held.tensors)
            key = next(iter(checkpoint.components[component].tensors))
            into = bytearray(16)
            checkpoint.components[component].tensors.read_into(key, memoryview(into), role="", offset=0)
            return f"read {component}/{key} of {checkpoint.id}"
        except Exception as e:
            return f"{type(e).__name__}: {e}"[:300]


def _opened(path: str) -> str:
    """`opened` when this process may read it (its memory: ptrace access), else the refusal."""
    try:
        with open(path, "rb"):
            return "opened"
    except OSError as e:
        return type(e).__name__


def _tensord() -> int:
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as f:
                cmdline = f.read().replace(b"\0", b" ")
        except OSError:
            continue
        if b"machine serve" in cmdline or cmdline.split(b" ")[0].endswith(b"tensord"):
            return int(pid)
    return 0


@app.entrypoint(accelerator=False)
def scan(payload: Probe, weights: Weights) -> Report:
    time.sleep(payload.seconds)
    try:
        component = weights.components()[0]
        key = weights.keys(component)[0]
        into = bytearray(16)
        weights.read_into(component, key, "", 0, into)
        read = f"{component}/{key}: {len(into)} bytes"
    except Exception as e:
        read = f"{type(e).__name__}: {e}"[:300]
    held = {}
    for obj in gc.get_objects():
        if type(obj).__name__ == "Bearers":
            held.update(getattr(obj, "_latest", {}))
    endpoints = sorted({endpoint for (_, endpoint) in held})
    tokens = sorted({token for (token, _) in held.values()})
    jwts, keys = _scan()
    environment = [f"{k}={v}" for k, v in os.environ.items()]
    named = sorted(k for k in os.environ if re.search(r"token|secret|dpop|capab|bearer|private", k, re.I))
    tensord = _tensord()
    given = endpoints and tokens and payload.ref
    through = _open(payload.ref, endpoints[0], tokens[0]) if given else "nothing given"
    direct = _open(payload.ref, payload.hub, tokens[0]) if given else "nothing given"
    return Report(
        endpoints=endpoints,
        tokens=[f"{len(t)} characters, {'a JWT' if JWT.search(t) else 'not a JWT'}" for t in tokens],
        jwts=jwts + sum(1 for s in environment if JWT.search(s)),
        private_keys=keys + sum(1 for s in environment if PRIVATE.search(s)),
        secrets_in_environment=named,
        tensord_memory=_opened(f"/proc/{tensord}/mem") if tensord else "no TensorD process found",
        tensord_environment=_opened(f"/proc/{tensord}/environ") if tensord else "no TensorD process found",
        opened_through_endpoint=through,
        opened_at_hub_with_token=direct,
        weights_read=read,
    )
