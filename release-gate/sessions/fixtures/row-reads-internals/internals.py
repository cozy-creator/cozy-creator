"""row-reads' TensorFS half (tfs-303; sessions/row-reads.sh), run as root on the pod with an executor environment's Python: its
torch, and the TensorFS the machine runs. A package can't run this: the package contract refuses `import tensorfs`, and a CUDA
out (torch's `__dlpack__(stream=)` makes a `torch.cuda.ExternalStream`: own_stream).

    python internals.py <checkpoint id> <seed> <per scheme> <model ref>    (TENSORFS_HOME: the machine's store, which holds it)

On the checkpoint fixtures/row-reads/rows.py just read through `cozy run`, a sample per scheme by row ranges with
`Checkpoint.read_rows`: CUDA outs on the caller's stream, each part equal byte for byte to the same rows of a whole read, and
each encoded range decoded alone (`raw.decode_into`, the CPU reference) equal to the same rows of the whole decode; one range
of each batched into one read on the caller's stream and on none. The largest tensor's read speed into CUDA: whole, one read
of every row, eight windows. Then a varena Source of a block of its tensors, as the Runtime places one, with the largest
encoded tensor's largest part across one of its page's registered windows (`WINDOW`, varena/pages.rs): every tensor's rows
and that tensor's windows by `Source.read_rows`, from the store, then from its kept page, into zeroed outs, every byte from
where it should. Prints one JSON report."""

import json
import math
import random
import statistics
import sys
import time
from collections.abc import Callable

import numpy as np
import tensorfs
import torch
from tensorfs import raw
from tensorfs.varena import _runtime as varena

WINDOWS = 8
#: a kept page is registered with CUDA in runs of this many bytes, and no DMA may span two (`WINDOW`, varena/pages.rs)
PAGE_WINDOW = 64 << 20
ALIGN = 256
#: an encoded tensor is decode-checked up to this many elements (the CPU reference decoder)
DECODE_ELEMENTS = 1 << 25
LOGICAL = {"BF16": torch.bfloat16, "F16": torch.float16, "F32": torch.float32}


def scheme_of(meta) -> str:
    return meta.encoding.scheme if meta.encoding is not None else "plain"


def expected(part, whole: bytes, count: int, tile: int, start: int, stop: int) -> bytes:
    """As fixtures/row-reads/rows.py: `part`'s bytes for rows [start, stop) of a whole read, from its stored shape."""
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


def main() -> None:
    ckid, seed, per_scheme, ref = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    # By its digest, else by its model reference: either way only from the machine's store.
    try:
        ck, opened = tensorfs.open(ckid, local_files_only=True), ckid
    except Exception as error:  # noqa: BLE001
        ck, opened = tensorfs.open(ref, local_files_only=True), f"{ref} ({ckid}: {type(error).__name__}: {error})"[:300]
    rng = random.Random(seed)
    # A stream of the caller's own, as the Runtime reads on: every read and check below is ordered on it.
    torch.cuda.set_stream(torch.cuda.Stream())
    stream = torch.cuda.current_stream().cuda_stream
    report: dict = {"tensorfs": tensorfs.__version__, "checkpoint": ck.id, "opened": opened, "lines": [], "schemes": {}}
    lines: list[str] = report["lines"]
    failed = False

    def fail(text: str) -> None:
        nonlocal failed
        failed = True
        lines.append(f"FAIL {text}")

    candidates: dict[str, list] = {}
    for cname, comp in ck.components.items():
        for key, meta in comp.tensors.items():
            cut = ck.rows(cname, key)
            if cut is not None:
                size = sum(p.length for p in raw.parts(ck, cname, key).values())
                candidates.setdefault(scheme_of(meta), []).append((size, cname, key, meta, cut))
    batch, batch_want, largest = [], [], None
    for scheme, found in sorted(candidates.items()):
        found.sort(key=lambda c: c[0], reverse=True)
        picked = found[:2] + rng.sample(found[2:], min(len(found) - 2, max(0, per_scheme - 2))) if len(found) > 2 else found
        if largest is None or found[0][0] > largest[0]:
            largest = found[0]
        row = report["schemes"].setdefault(scheme, {"tensors": 0, "ranges": 0, "decoded": 0, "too_large": 0})
        for _, cname, key, meta, cut in picked:
            try:
                tensors = ck.components[cname].tensors
                stored = raw.parts(ck, cname, key)
                whole = whole_of(ck, cname, key, meta)
                small = meta.encoding is not None and math.prod(meta.shape) <= DECODE_ELEMENTS
                row["too_large"] += meta.encoding is not None and not small
                decoded = tensors.read(key, decode=True).tobytes() if small else None
                row["tensors"] += 1
                for start, stop in ranges(cut, rng):
                    n, sizes = stop - start, cut.nbytes(start, stop)
                    gpu = {r: torch.zeros(m, dtype=torch.uint8, device="cuda") for r, m in sizes.items()}
                    ck.read_rows([(cname, key, start, stop, gpu)], stream=stream)
                    host = {r: g.cpu().numpy().tobytes() for r, g in gpu.items()}
                    row["ranges"] += 1
                    for role in sizes:
                        if host[role] != expected(stored[role], whole[role], cut.count, cut.tile, start, stop):
                            fail(f"{scheme} {cname}/{key}#{role} rows {start}..{stop}: CUDA bytes differ from the whole read's")
                    if decoded is None:
                        continue
                    out = torch.empty([n, *meta.shape[1:]], dtype=LOGICAL[meta.dtype])
                    parts = {r: (stored[r].dtype, sub_shape(stored[r], cut.count, cut.tile, n), bytearray(host[r])) for r in sizes}
                    raw.decode_into(meta.encoding.scheme, parts, out, dtype=meta.dtype, shape=[n, *meta.shape[1:]],
                                    params=dict(meta.encoding.params))
                    row["decoded"] += 1
                    per = len(decoded) // cut.count
                    if out.view(torch.uint8).numpy().tobytes() != decoded[start * per : stop * per]:
                        fail(f"{scheme} {cname}/{key} rows {start}..{stop}: its decode differs from the whole decode's rows")
                some = ranges(cut, rng)
                start, stop = some[min(2, len(some) - 1)]
                sizes = cut.nbytes(start, stop)
                batch.append((cname, key, start, stop, {r: torch.zeros(m, dtype=torch.uint8, device="cuda") for r, m in sizes.items()}))
                batch_want.append({r: expected(stored[r], whole[r], cut.count, cut.tile, start, stop) for r in sizes})
            except Exception as error:  # noqa: BLE001
                fail(f"{scheme} {cname}/{key}: {type(error).__name__}: {error}"[:400])
    for given in (stream, None):
        if not batch:
            break
        try:
            ck.read_rows(batch) if given is None else ck.read_rows(batch, stream=given)
            torch.cuda.synchronize()
            for b, want in zip(batch, batch_want):
                for role, w in want.items():
                    if b[4][role].cpu().numpy().tobytes() != w:
                        fail(f"batched{'' if given else ', no stream given,'} {b[0]}/{b[1]}#{role} rows {b[2]}..{b[3]}: "
                             "bytes differ from the whole read's")
                    b[4][role].zero_()
            torch.cuda.synchronize()
        except Exception as error:  # noqa: BLE001
            fail(f"batched{'' if given else ', no stream given'}: {type(error).__name__}: {error}"[:400])
    if batch:
        lines.append(f"batched: {len(batch)} tensors in one read_rows into CUDA, on the caller's stream and on none")
    for scheme, row in sorted(report["schemes"].items()):
        bad = any(line.startswith(f"FAIL {scheme} ") for line in lines)
        verdict = "FAIL" if bad else "PASS" if row["ranges"] else "UNCHECKED"
        lines.append(f"{verdict} {scheme}: {row['ranges']} ranges of {row['tensors']} tensors into CUDA on the caller's stream, "
                     f"{row['decoded']} decoded alone equal to the whole decode's rows ({row['too_large']} over "
                     f"{DECODE_ELEMENTS} elements not decoded)")
    if largest is not None:
        try:
            report["speed"] = speed(ck, largest, stream, fail)
        except Exception as error:  # noqa: BLE001
            fail(f"speed: {type(error).__name__}: {error}"[:400])
    every = [c for found in candidates.values() for c in found]
    if every:
        report["block"] = blk = block(ck, every, stream, fail)
        verdict = "FAIL" if any(line.startswith("FAIL block") for line in lines) else "PASS"
        lines.append(f"{verdict} block: {blk['tensors']} tensors, {blk['bytes']} bytes, {blk['target']} "
                     f"({blk['scheme']}) #{blk['part']} at {blk['part_at']} across a {blk['window']}-byte page window, "
                     f"{blk.get('windows_across', 0)} of its windows across it; served {blk.get('store', {}).get('served')} "
                     f"from the store, {blk.get('page', {}).get('served')} from its kept page (page, store)")
    report["failed"] = failed
    print(json.dumps(report))


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


if __name__ == "__main__":
    main()
