# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.48.0.dev0", "msgspec>=0.19", "numpy>=1.26", "torch>=2.14"]
# ///
"""row-reads' check (tfs-303), on one checkpoint: for every tensor that cuts (`Checkpoint.rows`), a sample per scheme read by
row ranges with `Checkpoint.read_rows` into host outs (plain buffers, DLPack arrays) and CUDA outs (on the caller's stream,
on none), alone and batched, one part without the others, each equal byte for byte to the same rows of a whole read
(`Tensors.read`), the rows told apart from the part's stored shape here, not from TensorFS's cut; each encoded range decoded
alone (`raw.decode_into`, the CPU reference) equal to the same rows of the whole decode. Then the largest tensor: its read
speed whole (`Tensors.read_into` per part), as one `read_rows` of every row and as eight windows. Then a varena Source of a
block of real tensors, as the Runtime places one, with the largest encoded tensor's largest part across one of its page's
registered windows (`WINDOW`, `varena/pages.rs`): every tensor's rows and that tensor's windows by `Source.read_rows`, from
the store and from its kept page, each reader's bytes checked into zeroed outs. `seed` makes each call its own."""

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
#: a kept page is registered with CUDA in runs of this many bytes, and no DMA may span two (`WINDOW`, varena/pages.rs)
PAGE_WINDOW = 64 << 20
ALIGN = 256
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
                if decoded is not None:
                    n = stop - start
                    per = len(decoded) // cut.count
                    out = torch.empty([n, *meta.shape[1:]], dtype=LOGICAL[meta.dtype])
                    parts = {r: (stored[r].dtype, sub_shape(stored[r], cut.count, cut.tile, n), host[r]) for r in sizes}
                    raw.decode_into(meta.encoding.scheme, parts, out, dtype=meta.dtype, shape=[n, *meta.shape[1:]],
                                    params=dict(meta.encoding.params))
                    if out.view(torch.uint8).numpy().tobytes() != decoded[start * per : stop * per]:
                        fail(f"{scheme} {cname}/{key} rows {start}..{stop}: its decode differs from the whole decode's rows")
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
        ck.read_rows(batch_gpu, stream=stream)
        for h, g, want in zip(batch_host, batch_gpu, batch_want):
            for role, w in want.items():
                if h[4][role].tobytes() != w or g[4][role].cpu().numpy().tobytes() != w:
                    fail(f"batched {h[0]}/{h[1]}#{role} rows {h[2]}..{h[3]}: bytes differ from the whole read's")
                g[4][role].zero_()
        # The CUDA outs again with no stream given: each written on a stream of TensorFS's own, landed when it returns.
        torch.cuda.synchronize()
        ck.read_rows(batch_gpu)
        for g, want in zip(batch_gpu, batch_want):
            for role, w in want.items():
                if g[4][role].cpu().numpy().tobytes() != w:
                    fail(f"batched, no stream given, {g[0]}/{g[1]}#{role} rows {g[2]}..{g[3]}: bytes differ from the whole read's")
        lines.append(f"batched: {len(batch_host)} tensors in one read_rows, host and CUDA, on the caller's stream and on none")
    if largest is not None:
        report["speed"] = speed(ck, largest, stream, fail)
        every = [c for found in candidates.values() for c in found]
        report["block"] = blk = block(ck, every, stream, fail)
        verdict = "FAIL" if any(line.startswith("FAIL block") for line in lines) else "PASS"
        lines.append(f"{verdict} block: {blk['tensors']} tensors, {blk['bytes']} bytes, {blk['target']} "
                     f"({blk['scheme']}) #{blk['part']} at {blk['part_at']} across a {blk['window']}-byte page window, "
                     f"{blk.get('windows_across', 0)} of its windows across it; served {blk.get('store', {}).get('served')} "
                     f"from the store, {blk.get('page', {}).get('served')} from its kept page (page, store)")
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
    stored = raw.parts(ck, cname, key)
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


def whole_of(ck, cname: str, key: str, meta) -> dict[str, bytes]:
    """Each part's bytes, by role (`""`: a plain tensor's data), from a whole read."""
    read = ck.components[cname].tensors.read(key, decode=False)
    return {"": read.tobytes()} if meta.encoding is None else {r: a.tobytes() for r, a in read.parts.items()}


def block(ck, every: list, stream: int, fail: Callable[[str], None]) -> dict:
    """A varena Source of a block of real tensors at aligned offsets, as the Runtime places one, with the largest encoded
    tensor's largest part across one of its page's windows: every tensor's rows in one read (host, then CUDA) and that
    tensor's windows one read each, from the store, then from its kept page. Each reader's bytes are checked into zeroed
    outs, and every byte must come from where it should: all from the store, then all from the page."""
    up = lambda n: -(-n // ALIGN) * ALIGN  # noqa: E731
    encoded = [c for c in every if c[3].encoding is not None] or every
    target = max(encoded, key=lambda c: max(p.length for p in raw.parts(ck, c[1], c[2]).values()))
    _, tc, tk, tmeta, tcut = target
    tparts = sorted(raw.parts(ck, tc, tk).items(), key=lambda rp: rp[1].length, reverse=True)
    big_role, big = tparts[0][0], tparts[0][1].length
    aim = (PAGE_WINDOW - big // 2) // ALIGN * ALIGN
    placed, parts, offset = [], [], 0

    def place(c, items) -> None:
        nonlocal offset
        for role, p in items:
            parts.append((f"{c[1]}/{c[2]}#{role or 'value'}", offset, p.length))
            offset += up(p.length)
        placed.append(c)

    # Other tensors before it, the largest that fit first, so its largest part starts about half its length short of a window.
    for c in sorted((c for c in every if c is not target), key=lambda c: c[0], reverse=True):
        items = list(raw.parts(ck, c[1], c[2]).items())
        if offset + sum(up(p.length) for _, p in items) <= aim:
            place(c, items)
    at = offset
    place(target, tparts)
    found = {"tensors": len(placed), "bytes": offset, "target": f"{tc}/{tk}", "scheme": scheme_of(tmeta),
             "part": big_role or "value", "part_at": [at, at + big], "window": PAGE_WINDOW}
    if not at < PAGE_WINDOW < at + big:
        fail(f"block: {tc}/{tk}#{big_role} at {at}..{at + big} lies inside one page window (too few tensors before it)")
    stored = {id(c): raw.parts(ck, c[1], c[2]) for c in placed}
    known = {id(c): whole_of(ck, c[1], c[2], c[3]) for c in placed}
    host = [(c[1], c[2], 0, c[4].count, {r: np.zeros(n, np.uint8) for r, n in c[4].nbytes(0, c[4].count).items()}) for c in placed]
    gpu = [(c[1], c[2], 0, c[4].count, {r: torch.zeros(n, dtype=torch.uint8, device="cuda")
                                        for r, n in c[4].nbytes(0, c[4].count).items()}) for c in placed]
    per = -(-tcut.count // WINDOWS)
    step = -(-per // tcut.tile) * tcut.tile
    bounds = [(a, min(tcut.count, a + step)) for a in range(0, tcut.count, step)]
    wins = [(tc, tk, a, b, {r: torch.zeros(n, dtype=torch.uint8, device="cuda") for r, n in tcut.nbytes(a, b).items()})
            for a, b in bounds]
    tile_bytes = tcut.tile_bytes[big_role]
    found["windows_across"] = sum(at + (a // tcut.tile) * tile_bytes < PAGE_WINDOW < at + (a // tcut.tile) * tile_bytes
                                  + tcut.nbytes(a, b)[big_role] for a, b in bounds) if tile_bytes else 0
    total = sum(n for reads in (host, gpu, wins) for _, _, _, _, outs in reads for n in (o.nbytes for o in outs.values()))
    by_id = {(c[1], c[2]): c for c in placed}

    def landed(label: str, readers: tuple = (host, gpu, wins), check: bool = True) -> None:
        torch.cuda.synchronize()
        for reads in readers:
            for cname, key, a, b, outs in reads:
                c = by_id[(cname, key)]
                for role, out in outs.items():
                    got = out.tobytes() if isinstance(out, np.ndarray) else out.cpu().numpy().tobytes()
                    want = expected(stored[id(c)][role], known[id(c)][role], c[4].count, c[4].tile, a, b)
                    if check and got != want:
                        fail(f"block from {label}: {cname}/{key}#{role} rows {a}..{b}: bytes differ from the whole read's")
                    out.fill(0) if isinstance(out, np.ndarray) else out.zero_()
        torch.cuda.synchronize()

    def read_all(source) -> tuple[int, int]:
        page = store = 0
        for reads, s in ((host, None), (gpu, stream)):
            got = source.read_rows(reads, stream=s)
            page, store = page + got[0], store + got[1]
        for one in wins:
            got = source.read_rows([one], stream=stream)
            page, store = page + got[0], store + got[1]
        return page, store

    def timings(label: str, source) -> dict:
        s_all = timed(lambda: source.read_rows(gpu, stream=stream))
        s_win = timed(lambda: [source.read_rows([one], stream=stream) for one in wins])
        landed(f"{label}, timed", (gpu, wins))
        return {"all_rows_s": round(s_all, 4), "all_rows_gb_s": round(offset / s_all / 1e9, 2),
                "target_windows_s": round(s_win, 4)}

    try:
        found["checkpoint"] = timings("the checkpoint", ck)
        pages = varena.Pages(torch.cuda.current_device())
        source = varena.Source(pages, "row-reads", 0, offset, checkpoint=ck, parts=parts, page=f"row-reads:{ck.id}:block")
        served = read_all(source)
        landed("the store")
        if served != (0, total):
            fail(f"block before its page is kept: served {served} (page, store), not (0, {total})")
        found["store"] = {"served": served, **timings("the store", source)}
        pages.read_ahead([source])
        until = time.monotonic() + 120
        while time.monotonic() < until and source.read_rows(wins[:1], stream=stream)[0] == 0:
            time.sleep(0.2)
        landed("", check=False)
        served = read_all(source)
        landed("its kept page")
        if served != (total, 0):
            fail(f"block from its kept page: served {served} (page, store), not ({total}, 0)")
        found["page"] = {"served": served, **timings("its kept page", source)}
        pages.forget("row-reads")
    except Exception as error:
        fail(f"block: {type(error).__name__}: {error}"[:400])
    return found
