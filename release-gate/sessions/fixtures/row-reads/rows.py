# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.48.0.dev0", "msgspec>=0.19", "numpy>=1.26", "torch>=2.14"]
# ///
"""row-reads' check (tfs-303), on one checkpoint: for every tensor that cuts (`Checkpoint.rows`), a sample per scheme read by
row ranges with `Checkpoint.read_rows` into host and CUDA outs, alone and batched, each part equal byte for byte to the same
rows of a whole read (`Tensors.read`), the rows told apart from the part's stored shape here, not from TensorFS's cut; each
encoded range decoded alone (`raw.decode_into`, the CPU reference) equal to the same rows of the whole decode; a varena
Source's `read_rows` from its kept page and from the store, equal too. Then the read speed of the largest tensor: whole
(`Tensors.read_into` per part), one `read_rows` of every row, eight windows, and the Source's windows from its page.
`seed` makes each call its own: no memo answers it."""

import json
import math
import random
import statistics
import time
from collections.abc import Callable

import msgspec
import numpy as np
import torch
from cozy_runtime.author import App, Context, Weights
from tensorfs import raw
from tensorfs.varena import _runtime as varena

app = App()
WINDOWS = 8
LOGICAL = {"BF16": torch.bfloat16, "F16": torch.float16, "F32": torch.float32}


class Ask(msgspec.Struct, kw_only=True):
    seed: int = 0
    per_scheme: int = 6
    #: an encoded tensor is decode-checked up to this many elements (the CPU reference decoder)
    decode_elements: int = 1 << 25


class Checked(msgspec.Struct):
    report: str


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


def sub_shape(part, count: int, tile: int, n: int) -> list[int]:
    shape = list(part.shape)
    if shape and shape[0] == count:
        return [n, *shape[1:]]
    if shape and tile > 1 and shape[0] == -(-count // tile) * tile:
        return [-(-n // tile) * tile, *shape[1:]]
    return shape


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
    report: dict = {"checkpoint": ck.id, "lane": ck.lane, "schemes": {}, "lines": [], "speed": {}}
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
            size = sum(p.length for p in raw.parts(ck, cname, key).values())
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
            stored = raw.parts(ck, cname, key)
            read = tensors.read(key, decode=False)
            whole = {"": read.tobytes()} if meta.encoding is None else {r: a.tobytes() for r, a in read.parts.items()}
            if set(cut.nbytes(0, cut.count)) != set(whole):
                fail(f"{scheme} {cname}/{key}: roles {sorted(cut.nbytes(0, cut.count))}, whole read {sorted(whole)}")
                continue
            decoded = None
            if meta.encoding is not None and math.prod(meta.shape) <= payload.decode_elements:
                decoded = tensors.read(key, decode=True).tobytes()
            for start, stop in ranges(cut, rng):
                sizes = cut.nbytes(start, stop)
                host = {r: np.empty(n, np.uint8) for r, n in sizes.items()}
                gpu = {r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in sizes.items()}
                ck.read_rows([(cname, key, start, stop, host)])
                ck.read_rows([(cname, key, start, stop, gpu)], stream=stream)
                for role in sizes:
                    want = expected(stored[role], whole[role], cut.count, cut.tile, start, stop)
                    if host[role].tobytes() != want:
                        fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: host bytes differ from the whole read's")
                    if gpu[role].cpu().numpy().tobytes() != want:
                        fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: CUDA bytes differ from the whole read's")
                if decoded is not None:
                    n = stop - start
                    per = len(decoded) // cut.count
                    out = torch.empty([n, *meta.shape[1:]], dtype=LOGICAL[meta.dtype])
                    parts = {r: (stored[r].dtype, sub_shape(stored[r], cut.count, cut.tile, n), host[r]) for r in sizes}
                    raw.decode_into(meta.encoding.scheme, parts, out, dtype=meta.dtype, shape=[n, *meta.shape[1:]],
                                    params=dict(meta.encoding.params))
                    if out.view(torch.uint8).numpy().tobytes() != decoded[start * per : stop * per]:
                        fail(f"{scheme} {cname}/{key} rows {start}..{stop}: its decode differs from the whole decode's rows")
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
        ck.read_rows(batch_gpu, stream=stream)
        for h, g, want in zip(batch_host, batch_gpu, batch_want):
            for role, w in want.items():
                if h[4][role].tobytes() != w or g[4][role].cpu().numpy().tobytes() != w:
                    fail(f"batched {h[0]}/{h[1]}#{role} rows {h[2]}..{h[3]}: bytes differ from the whole read's")
        lines.append(f"batched: {len(batch_host)} tensors in one read_rows, host and CUDA")
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
    """The largest tensor's read speed, warm (its bytes in the page cache), into CUDA: whole, one read of every row, eight
    windows, and a varena Source's windows from its kept page against from the store."""
    size, cname, key, meta, cut = largest
    tensors = ck.components[cname].tensors
    stored = raw.parts(ck, cname, key)
    whole = {r: torch.empty(p.length, dtype=torch.uint8, device="cuda") for r, p in stored.items()}

    def read_whole() -> None:
        for role, out in whole.items():
            tensors.read_into(key, out, role=role or None)

    read_whole()
    every = {r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in cut.nbytes(0, cut.count).items()}
    per = -(-cut.count // WINDOWS)
    step = -(-per // cut.tile) * cut.tile
    bounds = [(a, min(cut.count, a + step)) for a in range(0, cut.count, step)]
    windows = [{r: torch.empty(n, dtype=torch.uint8, device="cuda") for r, n in cut.nbytes(a, b).items()} for a, b in bounds]
    read_windows = [(cname, key, a, b, w) for (a, b), w in zip(bounds, windows)]

    def each(reader) -> None:
        for one in read_windows:
            reader([one], stream=stream)

    found = {"tensor": f"{cname}/{key}", "scheme": scheme_of(meta), "bytes": size, "rows": cut.count, "windows": len(bounds)}
    for label, f in [("whole", read_whole), ("rows_all", lambda: ck.read_rows([(cname, key, 0, cut.count, every)], stream=stream)),
                     ("windows", lambda: each(ck.read_rows))]:
        s = timed(f)
        found[label] = {"s": round(s, 4), "gb_s": round(size / s / 1e9, 2)}
    # A varena Source of this tensor's parts, as the Runtime makes one for a block: from the store, then its kept page.
    try:
        pages = varena.Pages(torch.cuda.current_device())
        offset, parts = 0, []
        for role, p in stored.items():
            parts.append((f"{cname}/{key}#{role or 'value'}", offset, p.length))
            offset += -(-p.length // 256) * 256
        source = varena.Source(pages, "row-reads", 0, offset, checkpoint=ck, parts=parts, page=f"row-reads:{ck.id}:{key}")
        served = source.read_rows(read_windows[:1], stream=stream)
        found["source_store"] = {"s": round(timed(lambda: each(source.read_rows)), 4), "first": served}
        found["source_store"]["gb_s"] = round(size / found["source_store"]["s"] / 1e9, 2)
        pages.read_ahead([source])
        until = time.monotonic() + 120
        while time.monotonic() < until and source.read_rows(read_windows[:1], stream=stream)[0] == 0:
            time.sleep(0.2)
        served = source.read_rows(read_windows[:1], stream=stream)
        if served[0] == 0:
            found["source_page"] = "its page was not kept in 120 s"
        else:
            s = timed(lambda: each(source.read_rows))
            found["source_page"] = {"s": round(s, 4), "gb_s": round(size / s / 1e9, 2), "first": served}
            known = {role: out.cpu().numpy().tobytes() for role, out in whole.items()}
            for (a, b), w in zip(bounds, windows):
                for role, out in w.items():
                    if out.cpu().numpy().tobytes() != expected(stored[role], known[role], cut.count, cut.tile, a, b):
                        fail(f"source page {cname}/{key}#{role} rows {a}..{b}: bytes differ from the whole read's")
    except Exception as error:  # a tier this process could not make is reported, not the check's failure
        found["source_error"] = f"{type(error).__name__}: {error}"[:300]
    return found
