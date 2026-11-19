"""row-reads' varena half (tfs-303; sessions/row-reads.sh), run as root on the pod with an executor environment's Python: its
torch, and the TensorFS the machine runs. No package can reach a varena Source (the contract refuses `import tensorfs`), so
this follows io-fault.sh; everything a package can call is fixtures/row-reads/rows.py's, through `cozy run`.

    python internals.py <checkpoint id> <model ref>    (TENSORFS_HOME: the machine's store, which holds it)

On the checkpoint rows.py just read: a varena Source of a block of its tensors, as the Runtime places one, with the largest
encoded tensor's largest part across one of its page's registered windows (`WINDOW`, varena/pages.rs): every tensor's rows
and that tensor's windows by `Source.read_rows`, from the store, then from its kept page, into zeroed outs, every byte from
where it should. Prints one JSON report."""

from __future__ import annotations

import json
import statistics
import sys
import time
from collections.abc import Callable, Sequence
from typing import TYPE_CHECKING, NamedTuple, cast

import numpy as np
import numpy.typing as npt
import tensorfs
import torch
from tensorfs import raw
from tensorfs.varena import _runtime as varena

if TYPE_CHECKING:
    from collections.abc import Buffer

    from tensorfs import Array, Checkpoint, Encoded, RowCut, TensorMetadata
    from tensorfs._ext import RowRead, _DLPackArray

WINDOWS = 8
#: a kept page is registered with CUDA in runs of this many bytes, and no DMA may span two (`WINDOW`, varena/pages.rs)
PAGE_WINDOW = 64 << 20
ALIGN = 256


class Candidate(NamedTuple):
    """A tensor that cuts: its stored bytes, where it is, and its cut."""

    size: int
    component: str
    key: str
    meta: TensorMetadata
    cut: RowCut


class HostRead(NamedTuple):
    component: str
    key: str
    start: int
    stop: int
    outs: dict[str, npt.NDArray[np.uint8]]


class CudaRead(NamedTuple):
    component: str
    key: str
    start: int
    stop: int
    outs: dict[str, torch.Tensor]


def rows_of(reads: Sequence[HostRead | CudaRead]) -> list[RowRead]:
    """`reads` as `read_rows` takes them: the same outs."""
    found: list[RowRead] = []
    for r in reads:
        outs: dict[str, Buffer | _DLPackArray] = dict(r.outs)
        found.append((r.component, r.key, r.start, r.stop, outs))
    return found


def scheme_of(meta: TensorMetadata) -> str:
    return meta.encoding.scheme if meta.encoding is not None else "plain"


def expected(part: raw.Part, whole: bytes, count: int, tile: int, start: int, stop: int) -> bytes:
    """As fixtures/row-reads/rows.py: `part`'s bytes for rows [start, stop) of a whole read, from its stored shape."""
    shape, length = tuple(part.shape), part.length
    if shape and shape[0] == count:
        per = length // count
        return whole[start * per : stop * per]
    if shape and tile > 1 and shape[0] == -(-count // tile) * tile:
        per = length // shape[0]
        return whole[start * per : -(-stop // tile) * tile * per]
    return whole


def timed(f: Callable[[], object], times: int = 3) -> float:
    took: list[float] = []
    for _ in range(times):
        torch.cuda.synchronize()
        t = time.perf_counter()
        f()
        torch.cuda.synchronize()
        took.append(time.perf_counter() - t)
    return statistics.median(took)


def main() -> None:
    ckid, ref = sys.argv[1], sys.argv[2]
    # By its digest, else by its model reference: either way only from the machine's store.
    try:
        ck, opened = tensorfs.open(ckid, local_files_only=True), ckid
    except Exception as error:  # noqa: BLE001
        ck, opened = tensorfs.open(ref, local_files_only=True), f"{ref} ({ckid}: {type(error).__name__}: {error})"[:300]
    # A stream of the caller's own, as the Runtime reads on: every read and check below is ordered on it.
    torch.cuda.set_stream(torch.cuda.Stream())
    stream = torch.cuda.current_stream().cuda_stream
    lines: list[str] = []
    report: dict[str, object] = {"tensorfs": tensorfs.__version__, "checkpoint": ck.id, "opened": opened, "lines": lines}
    failed = False

    def fail(text: str) -> None:
        nonlocal failed
        failed = True
        lines.append(f"FAIL {text}")

    candidates: dict[str, list[Candidate]] = {}
    for cname, comp in ck.components.items():
        for key, meta in comp.tensors.items():
            cut = ck.rows(cname, key)
            if cut is not None:
                size = sum(p.length for p in raw.parts(ck, cname, key).values())
                candidates.setdefault(scheme_of(meta), []).append(Candidate(size, cname, key, meta, cut))
    every = [c for found in candidates.values() for c in found]
    if every:
        blk = block(ck, every, stream, fail)
        report["block"] = blk
        verdict = "FAIL" if any(line.startswith("FAIL block") for line in lines) else "PASS"
        lines.append(f"{verdict} block: {blk['tensors']} tensors, {blk['bytes']} bytes, {blk['target']} "
                     f"({blk['scheme']}) #{blk['part']} at {blk['part_at']} across a {blk['window']}-byte page window, "
                     f"{blk.get('windows_across', 0)} of its windows across it; served {served_of(blk, 'store')} "
                     f"from the store, {served_of(blk, 'page')} from its kept page (page, store)")
    report["failed"] = failed
    print(json.dumps(report))


def served_of(blk: dict[str, object], where: str) -> object:
    phase = blk.get(where)
    return phase.get("served") if isinstance(phase, dict) else None


def whole_of(ck: Checkpoint, cname: str, key: str, meta: TensorMetadata) -> dict[str, bytes]:
    """Each part's bytes, by role (`""`: a plain tensor's data), from a whole read."""
    got = ck.components[cname].tensors.read(key, decode=False)
    if meta.encoding is None:
        return {"": cast("Array", got).tobytes()}
    return {r: a.tobytes() for r, a in cast("Encoded", got).parts.items()}


def up(n: int) -> int:
    return -(-n // ALIGN) * ALIGN


def block(ck: Checkpoint, every: list[Candidate], stream: int, fail: Callable[[str], None]) -> dict[str, object]:
    """A varena Source of a block of real tensors at aligned offsets, as the Runtime places one, with the largest encoded
    tensor's largest part across one of its page's windows: every tensor's rows in one read (host, then CUDA) and that
    tensor's windows one read each, from the store, then from its kept page. Each reader's bytes are checked into zeroed
    outs, and every byte must come from where it should: all from the store, then all from the page."""
    encoded = [c for c in every if c.meta.encoding is not None] or every
    target = max(encoded, key=lambda c: max(p.length for p in raw.parts(ck, c.component, c.key).values()))
    tc, tk, tmeta, tcut = target.component, target.key, target.meta, target.cut
    tparts = sorted(raw.parts(ck, tc, tk).items(), key=lambda rp: rp[1].length, reverse=True)
    big_role, big = tparts[0][0], tparts[0][1].length
    aim = (PAGE_WINDOW - big // 2) // ALIGN * ALIGN
    placed: list[Candidate] = []
    parts: list[tuple[str, int, int]] = []
    offset = 0

    def place(c: Candidate, items: list[tuple[str, raw.Part]]) -> None:
        nonlocal offset
        for role, p in items:
            parts.append((f"{c.component}/{c.key}#{role or 'value'}", offset, p.length))
            offset += up(p.length)
        placed.append(c)

    # Other tensors before it, the largest that fit first, so its largest part starts about half its length short of a window.
    for c in sorted((c for c in every if c is not target), key=lambda c: c.size, reverse=True):
        items = list(raw.parts(ck, c.component, c.key).items())
        if offset + sum(up(p.length) for _, p in items) <= aim:
            place(c, items)
    at = offset
    place(target, tparts)
    found: dict[str, object] = {"tensors": len(placed), "bytes": offset, "target": f"{tc}/{tk}", "scheme": scheme_of(tmeta),
                                "part": big_role or "value", "part_at": [at, at + big], "window": PAGE_WINDOW}
    if not at < PAGE_WINDOW < at + big:
        fail(f"block: {tc}/{tk}#{big_role} at {at}..{at + big} lies inside one page window (too few tensors before it)")
    stored = {(c.component, c.key): raw.parts(ck, c.component, c.key) for c in placed}
    known = {(c.component, c.key): whole_of(ck, c.component, c.key, c.meta) for c in placed}
    host = [HostRead(c.component, c.key, 0, c.cut.count, {r: np.zeros(n, np.uint8) for r, n in c.cut.nbytes(0, c.cut.count).items()})
            for c in placed]
    gpu = [CudaRead(c.component, c.key, 0, c.cut.count, {r: torch.zeros(n, dtype=torch.uint8, device="cuda")
                                                         for r, n in c.cut.nbytes(0, c.cut.count).items()}) for c in placed]
    per = -(-tcut.count // WINDOWS)
    step = -(-per // tcut.tile) * tcut.tile
    bounds = [(a, min(tcut.count, a + step)) for a in range(0, tcut.count, step)]
    wins = [CudaRead(tc, tk, a, b, {r: torch.zeros(n, dtype=torch.uint8, device="cuda") for r, n in tcut.nbytes(a, b).items()})
            for a, b in bounds]
    tile_bytes = tcut.tile_bytes[big_role]
    found["windows_across"] = sum(at + (a // tcut.tile) * tile_bytes < PAGE_WINDOW < at + (a // tcut.tile) * tile_bytes
                                  + tcut.nbytes(a, b)[big_role] for a, b in bounds) if tile_bytes else 0
    all_reads: list[HostRead | CudaRead] = [*host, *gpu, *wins]
    cuda_reads: list[HostRead | CudaRead] = [*gpu, *wins]
    total = sum(o.nbytes for r in all_reads for o in r.outs.values())
    by_key = {(c.component, c.key): c for c in placed}
    host_rows, gpu_rows = rows_of(host), rows_of(gpu)
    win_rows = [rows_of([w]) for w in wins]

    def landed(label: str, readers: list[HostRead | CudaRead] = all_reads, check: bool = True) -> None:
        torch.cuda.synchronize()
        for r in readers:
            c = by_key[(r.component, r.key)]
            for role, out in r.outs.items():
                got = out.tobytes() if isinstance(out, np.ndarray) else out.cpu().numpy().tobytes()
                want = expected(stored[(r.component, r.key)][role], known[(r.component, r.key)][role], c.cut.count, c.cut.tile,
                                r.start, r.stop)
                if check and got != want:
                    fail(f"block from {label}: {r.component}/{r.key}#{role} rows {r.start}..{r.stop}: "
                         "bytes differ from the whole read's")
                if isinstance(out, np.ndarray):
                    out.fill(0)
                else:
                    out.zero_()
        torch.cuda.synchronize()

    def read_all(source: varena.Source) -> tuple[int, int]:
        page = store = 0
        for reads, s in ((host_rows, None), (gpu_rows, stream)):
            got = source.read_rows(reads, stream=s)
            page, store = page + got[0], store + got[1]
        for one in win_rows:
            got = source.read_rows(one, stream=stream)
            page, store = page + got[0], store + got[1]
        return page, store

    def timings(label: str, reader: Callable[[list[RowRead]], object]) -> dict[str, float]:
        s_all = timed(lambda: reader(gpu_rows))
        s_win = timed(lambda: [reader(one) for one in win_rows])
        landed(f"{label}, timed", cuda_reads)
        return {"all_rows_s": round(s_all, 4), "all_rows_gb_s": round(offset / s_all / 1e9, 2),
                "target_windows_s": round(s_win, 4)}

    try:
        found["checkpoint"] = timings("the checkpoint", lambda reads: ck.read_rows(reads, stream=stream))
        pages = varena.Pages(torch.cuda.current_device())
        source = varena.Source(pages, "row-reads", 0, offset, checkpoint=ck, parts=parts, page=f"row-reads:{ck.id}:block")
        served = read_all(source)
        landed("the store")
        if served != (0, total):
            fail(f"block before its page is kept: served {served} (page, store), not (0, {total})")
        found["store"] = {"served": served, **timings("the store", lambda reads: source.read_rows(reads, stream=stream))}
        pages.read_ahead([source])
        until = time.monotonic() + 120
        while time.monotonic() < until and source.read_rows(win_rows[0], stream=stream)[0] == 0:
            time.sleep(0.2)
        landed("", check=False)
        served = read_all(source)
        landed("its kept page")
        if served != (total, 0):
            fail(f"block from its kept page: served {served} (page, store), not ({total}, 0)")
        found["page"] = {"served": served, **timings("its kept page", lambda reads: source.read_rows(reads, stream=stream))}
        pages.forget("row-reads")
    except Exception as error:  # noqa: BLE001
        fail(f"block: {type(error).__name__}: {error}"[:400])
    return found


if __name__ == "__main__":
    main()
