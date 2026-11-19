# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.47.1", "msgspec>=0.19", "numpy>=1.26", "torch>=2.14"]
# ///
"""row-reads' check (tfs-303), on one checkpoint, as a package reads it: for every tensor that cuts (`Checkpoint.rows`), a
sample per scheme read by row ranges with `Checkpoint.read_rows` into host outs (plain buffers, DLPack arrays) and CUDA outs
(on the caller's stream, on none), alone and batched, one part without the others, each equal byte for byte to the same rows
of a whole read (`Tensors.read`), the rows told apart from the part's stored shape here, not from TensorFS's cut. The batched
CUDA reads are each queued behind the caller's stream's pending work that zeroes their outs: on the caller's stream they
must land after it, and with none given TensorFS must wait for it. Then the largest tensor's read speed into CUDA, whole
(`Tensors.read_into` per part), as one `read_rows` of every row and as eight windows. A varena Source's rows, which no
package can reach, are checked beside it by fixtures/row-reads-internals/internals.py. `seed` makes each call its own."""

import importlib.metadata
import json
import os
import random
import statistics
import time
from collections.abc import Callable
from typing import NamedTuple

import msgspec
import numpy as np
import torch
from cozy_runtime.author import App, Context, Weights

app = App()
WINDOWS = 8
#: a square matmul's side: the pending work the batched CUDA reads are queued behind
BUSY = 4096


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


def timed(f: Callable[[], object], times: int = 3) -> float:
    took = []
    for _ in range(times):
        torch.cuda.synchronize()
        t = time.perf_counter()
        f()
        torch.cuda.synchronize()
        took.append(time.perf_counter() - t)
    return statistics.median(took)


@app.entrypoint(accelerator=True)
def check(ctx: Context, payload: Ask, weights: Weights) -> Checked:
    ck = ctx.tensorfs_source(weights)
    rng = random.Random(payload.seed)
    stream = torch.cuda.current_stream().cuda_stream
    report: dict = {"checkpoint": ck.id, "lane": ck.lane, "tensorfs": importlib.metadata.version("tensorfs"),
                    "runtime": importlib.metadata.version("cozy-runtime"), "tensorfs_home": os.environ.get("TENSORFS_HOME"),
                    "schemes": {}, "lines": [], "speed": {}}
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
    batch_host, batch_gpu, batch_want = [], [], []
    largest = None
    for scheme, found in sorted(candidates.items()):
        found.sort(key=lambda c: c[0], reverse=True)
        picked = found[:2] + rng.sample(found[2:], min(len(found) - 2, max(0, payload.per_scheme - 2))) if len(found) > 2 else found
        if largest is None or found[0][0] > largest[0]:
            largest = found[0]
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
                gpu = {r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in sizes.items()}
                ck.read_rows([(cname, key, start, stop, host)])
                ck.read_rows([(cname, key, start, stop, gpu)], stream=stream)
                for role in sizes:
                    want = expected(stored[role], whole[role], cut.count, cut.tile, start, stop)
                    if bytes(host[role]) != want:
                        fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: host bytes differ from the whole read's")
                    if gpu[role].cpu().numpy().tobytes() != want:
                        fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: CUDA bytes differ from the whole read's")
            # A role left out is not read: the first tile's first part alone.
            start, stop = 0, min(cut.tile, cut.count)
            role, n = next(iter(cut.nbytes(start, stop).items()))
            alone = bytearray(n)
            ck.read_rows([(cname, key, start, stop, {role: alone})])
            if bytes(alone) != expected(stored[role], whole[role], cut.count, cut.tile, start, stop):
                fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: read alone, its bytes differ from the whole read's")
            # One range of each into the batch: every sample in one read, host and CUDA.
            some = ranges(cut, rng)
            start, stop = some[min(2, len(some) - 1)]
            sizes = cut.nbytes(start, stop)
            batch_host.append((cname, key, start, stop, {r: np.empty(n, np.uint8) for r, n in sizes.items()}))
            batch_gpu.append((cname, key, start, stop, {r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in sizes.items()}))
            batch_want.append({r: expected(stored[r], whole[r], cut.count, cut.tile, start, stop) for r in sizes})
            report["schemes"][scheme]["checked"] += 1
    if batch_host:
        ck.read_rows(batch_host)
        for h, want in zip(batch_host, batch_want):
            for role, w in want.items():
                if h[4][role].tobytes() != w:
                    fail(f"batched {h[0]}/{h[1]}#{role} rows {h[2]}..{h[3]}: host bytes differ from the whole read's")
        a = torch.ones(BUSY, BUSY, dtype=torch.bfloat16, device="cuda")
        b = torch.empty_like(a)
        torch.cuda.synchronize()
        for given, label in ((stream, "on the caller's stream"), (None, "no stream given")):
            # Pending work on the caller's stream, then its zeroing of every out: the read must land after both.
            for _ in range(16):
                torch.mm(a, a, out=b)
            for g in batch_gpu:
                for out in g[4].values():
                    out.zero_()
            began = time.perf_counter()
            ck.read_rows(batch_gpu) if given is None else ck.read_rows(batch_gpu, stream=given)
            returned = time.perf_counter() - began
            for g, want in zip(batch_gpu, batch_want):
                for role, w in want.items():
                    if g[4][role].cpu().numpy().tobytes() != w:
                        fail(f"batched CUDA, {label}, {g[0]}/{g[1]}#{role} rows {g[2]}..{g[3]}: bytes differ from the whole read's")
            report.setdefault("batched_returned_s", {})[label] = round(returned, 4)
        lines.append(f"batched: {len(batch_host)} tensors in one read_rows, host, and CUDA behind pending zeroing on the "
                     f"caller's stream and with none given; returned in {report['batched_returned_s']} s")
    if largest is not None:
        report["speed"] = speed(ck, largest, stream, fail)
    for scheme, row in sorted(report["schemes"].items()):
        # A scheme none of whose tensors cut was not checked: no verdict of it.
        verdict = "FAIL" if any(line.startswith(f"FAIL {scheme} ") for line in lines) else "PASS" if row["checked"] else "UNCUT"
        lines.append(f"{verdict} {scheme}: {row['checked']} checked of {row['cut']} that cut, {row['tensors']} tensors; "
                     f"tiles {row['tiles']}, read whole {row['whole_roles']}")
    report["failed"] = failed
    return Checked(report=json.dumps(report))


def speed(ck, largest, stream: int, fail: Callable[[str], None]) -> dict:
    """The largest tensor's read speed, warm (its bytes in the page cache), into CUDA: whole, one read of every row and eight
    windows, the windows checked against the whole read."""
    size, cname, key, meta, cut = largest
    tensors = ck.components[cname].tensors
    _, stored = whole_of(tensors, key, meta)
    whole = {r: torch.empty(p.length, dtype=torch.uint8, device="cuda") for r, p in stored.items()}

    def read_whole() -> None:
        for role, out in whole.items():
            tensors.read_into(key, out, role=role or None)

    read_whole()
    known = {role: out.cpu().numpy().tobytes() for role, out in whole.items()}
    every = {r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in cut.nbytes(0, cut.count).items()}
    per = -(-cut.count // WINDOWS)
    step = -(-per // cut.tile) * cut.tile
    bounds = [(a, min(cut.count, a + step)) for a in range(0, cut.count, step)]
    windows = [{r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in cut.nbytes(a, b).items()} for a, b in bounds]
    read_windows = [(cname, key, a, b, w) for (a, b), w in zip(bounds, windows)]

    def each(reader) -> tuple[int, int]:
        """Every window, one read each: the bytes a Source served from its page and from the store."""
        page = store = 0
        for one in read_windows:
            got = reader([one], stream=stream)
            if got is not None:
                page, store = page + got[0], store + got[1]
        return page, store

    def landed(reader: str) -> None:
        """The windows hold their rows of the whole read; then zeroed, so what the next reader leaves is its own."""
        for (a, b), w in zip(bounds, windows):
            for role, out in w.items():
                if out.cpu().numpy().tobytes() != expected(stored[role], known[role], cut.count, cut.tile, a, b):
                    fail(f"{reader} {cname}/{key}#{role} rows {a}..{b}: bytes differ from the whole read's")
                out.zero_()
        torch.cuda.synchronize()

    found = {"tensor": f"{cname}/{key}", "scheme": scheme_of(meta), "bytes": size, "rows": cut.count, "windows": len(bounds)}
    for label, f in [("whole", read_whole), ("rows_all", lambda: ck.read_rows([(cname, key, 0, cut.count, every)], stream=stream)),
                     ("windows", lambda: each(ck.read_rows))]:
        s = timed(f)
        found[label] = {"s": round(s, 4), "gb_s": round(size / s / 1e9, 2)}
    landed("windows")
    return found

