# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.47.1", "msgspec>=0.19", "numpy>=1.26", "torch>=2.14"]  # torch: for internals.py, in this env
# ///
"""row-reads' check (tfs-303), on one checkpoint, as a package reads it: for every tensor that cuts (`Checkpoint.rows`), a
sample per scheme read by row ranges with `Checkpoint.read_rows` into host outs (plain buffers; DLPack arrays batched), alone
and batched, one part without the others, each equal byte for byte to the same rows of a whole read (`Tensors.read`), the rows
told apart from the part's stored shape here, not from TensorFS's cut. CUDA outs are not a package's: TensorFS hands a CUDA
out's stream to torch's `__dlpack__`, which makes a `torch.cuda.ExternalStream`, and the contract refuses it (own_stream,
session 44507401197). So TensorFS's side (CUDA outs on the caller's stream and on none, each range's decode, read speed, a
varena Source's rows) is checked beside it, outside a package, by fixtures/row-reads-internals/internals.py, as io-fault.sh
does; the contract also refuses `import tensorfs`. `seed` makes each call its own."""

import importlib.metadata
import json
import os
import random
from typing import NamedTuple

import msgspec
import numpy as np
from cozy_runtime.author import App, Context, Weights

app = App()


class Ask(msgspec.Struct, kw_only=True):
    seed: int = 0
    per_scheme: int = 6


class Checked(msgspec.Struct):
    report: str


class Part(NamedTuple):
    """A stored part's shape and length, as a whole read returns it."""

    shape: tuple
    length: int


def whole_of(tensors, key: str, meta) -> tuple[dict[str, bytes], dict[str, Part]]:
    """Each part's bytes and stored shape, by role (`""`: a plain tensor's data), from a whole read."""
    read = tensors.read(key, decode=False)
    arrays = {"": read} if meta.encoding is None else dict(read.parts.items())
    data = {r: a.tobytes() for r, a in arrays.items()}
    return data, {r: Part(tuple(a.shape), len(data[r])) for r, a in arrays.items()}


def scheme_of(meta) -> str:
    return meta.encoding.scheme if meta.encoding is not None else "plain"


def expected(part, whole: bytes, count: int, tile: int, start: int, stop: int) -> bytes:
    """`part`'s bytes for rows [start, stop) of a whole read, from its stored shape: rows lead, or lead in padded tiles, or
    nothing in it follows the rows (read whole)."""
    shape, length = tuple(part.shape), part.length
    if shape and shape[0] == count:
        per = length // count
        return whole[start * per : stop * per]
    if shape and tile > 1 and shape[0] == -(-count // tile) * tile:
        per = length // shape[0]
        return whole[start * per : -(-stop // tile) * tile * per]
    return whole


def ranges(cut, rng: random.Random) -> list[tuple[int, int]]:
    count, tile = cut.count, cut.tile
    tiles = -(-count // tile)
    last = (tiles - 1) * tile
    a = rng.randrange(tiles) * tile
    b = min(count, a + tile * rng.randint(1, max(1, tiles // 4)))
    found = [(0, min(tile, count)), (last, count), (a, b), (0, count)]
    if tile == 1 and count > 2:
        r = rng.randrange(1, count - 1)
        found.append((r, r + 1))
    return list(dict.fromkeys(found))


@app.entrypoint(accelerator=True)
def check(ctx: Context, payload: Ask, weights: Weights) -> Checked:
    ck = ctx.tensorfs_source(weights)
    rng = random.Random(payload.seed)
    report: dict = {"checkpoint": ck.id, "lane": ck.lane, "tensorfs": importlib.metadata.version("tensorfs"),
                    "runtime": importlib.metadata.version("cozy-runtime"), "tensorfs_home": os.environ.get("TENSORFS_HOME"),
                    "schemes": {}, "lines": []}
    lines: list[str] = report["lines"]
    failed = False

    def fail(text: str) -> None:
        nonlocal failed
        failed = True
        lines.append(f"FAIL {text}")

    # The census: every tensor, whether it cuts, by scheme.
    candidates: dict[str, list] = {}
    for cname, comp in ck.components.items():
        for key, meta in comp.tensors.items():
            scheme = scheme_of(meta)
            row = report["schemes"].setdefault(scheme, {"tensors": 0, "cut": 0, "tiles": {}, "whole_roles": [], "checked": 0})
            row["tensors"] += 1
            cut = ck.rows(cname, key)
            if cut is None:
                continue
            row["cut"] += 1
            row["tiles"][str(cut.tile)] = row["tiles"].get(str(cut.tile), 0) + 1
            for role, per in cut.tile_bytes.items():
                if per is None and role not in row["whole_roles"]:
                    row["whole_roles"].append(role)
            size = sum(cut.nbytes(0, cut.count).values())
            candidates.setdefault(scheme, []).append((size, cname, key, meta, cut))
    batch_host, batch_want = [], []
    for scheme, found in sorted(candidates.items()):
        found.sort(key=lambda c: c[0], reverse=True)
        picked = found[:2] + rng.sample(found[2:], min(len(found) - 2, max(0, payload.per_scheme - 2))) if len(found) > 2 else found
        for size, cname, key, meta, cut in picked:
            tensors = ck.components[cname].tensors
            whole, stored = whole_of(tensors, key, meta)
            if set(cut.nbytes(0, cut.count)) != set(whole):
                fail(f"{scheme} {cname}/{key}: roles {sorted(cut.nbytes(0, cut.count))}, whole read {sorted(whole)}")
                continue
            for start, stop in ranges(cut, rng):
                sizes = cut.nbytes(start, stop)
                # Host outs as plain buffers here; as DLPack arrays (numpy) in the batched read below.
                host = {r: bytearray(n) for r, n in sizes.items()}
                ck.read_rows([(cname, key, start, stop, host)])
                for role in sizes:
                    if bytes(host[role]) != expected(stored[role], whole[role], cut.count, cut.tile, start, stop):
                        fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: host bytes differ from the whole read's")
            # A role left out is not read: the first tile's first part alone.
            start, stop = 0, min(cut.tile, cut.count)
            role, n = next(iter(cut.nbytes(start, stop).items()))
            alone = bytearray(n)
            ck.read_rows([(cname, key, start, stop, {role: alone})])
            if bytes(alone) != expected(stored[role], whole[role], cut.count, cut.tile, start, stop):
                fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: read alone, its bytes differ from the whole read's")
            # One range of each into the batch: every sample in one read.
            some = ranges(cut, rng)
            start, stop = some[min(2, len(some) - 1)]
            sizes = cut.nbytes(start, stop)
            batch_host.append((cname, key, start, stop, {r: np.empty(n, np.uint8) for r, n in sizes.items()}))
            batch_want.append({r: expected(stored[r], whole[r], cut.count, cut.tile, start, stop) for r in sizes})
            report["schemes"][scheme]["checked"] += 1
    if batch_host:
        ck.read_rows(batch_host)
        for h, want in zip(batch_host, batch_want):
            for role, w in want.items():
                if h[4][role].tobytes() != w:
                    fail(f"batched {h[0]}/{h[1]}#{role} rows {h[2]}..{h[3]}: bytes differ from the whole read's")
        lines.append(f"batched: {len(batch_host)} tensors in one read_rows into DLPack host arrays")
    for scheme, row in sorted(report["schemes"].items()):
        # A scheme none of whose tensors cut was not checked: no verdict of it.
        verdict = "FAIL" if any(line.startswith(f"FAIL {scheme} ") for line in lines) else "PASS" if row["checked"] else "UNCUT"
        lines.append(f"{verdict} {scheme}: {row['checked']} checked of {row['cut']} that cut, {row['tensors']} tensors; "
                     f"tiles {row['tiles']}, read whole {row['whole_roles']}")
    report["failed"] = failed
    return Checked(report=json.dumps(report))

