"""tfs-bound's pod side (sessions/tfs-bound.sh), run as root over `cozy machine ssh` with an executor environment's Python:
its torch, and the TensorFS the machine runs. A package can't reach a varena Source (the contract refuses `import tensorfs`),
so this follows io-fault.sh; the same models also run through `cozy run`.

    python load.py <mode> <args>   (TENSORFS_HOME: the machine's store)

  ceiling                TensorFS's own ceilings: disk GB/s of the store's volume, pinned H2D GB/s to GPU 0
  evict <ck>...          the checkpoints' chunk files out of the page cache
  disk <ck>...           the disk itself, no TensorFS: the checkpoints' chunk files read O_DIRECT on a grid of threads and
                         request sizes (each on files no other read touched), then buffered from an empty page cache
  fill <ck> <comps>      a whole-resident load the way the Runtime binds one (cr-318): one Region a block, every block's
                         Source over its stored parts, all filled by one `varena.fill`; from disk (evicted), then from RAM
  paged <ck> <comp>      a paged model's host tier: its blocks' pages read ahead from disk into pinned RAM (`Pages.read_ahead`),
                         each read whole into pinned RAM by its Source (`read_host`), then filled onto the card from those pages

Prints one JSON object. Rates are GB/s (1e9 bytes).
"""

from __future__ import annotations

import concurrent.futures
import gc
import glob
import json
import mmap
import os
import sys
import time
from collections.abc import Sequence

GB = 1e9
DIRECT_GRID = [(8, 1 << 20), (8, 4 << 20), (16, 2 << 20), (16, 4 << 20), (32, 4 << 20)]
BUFFERED_GRID = [(8, 1 << 20), (16, 4 << 20)]


def store() -> str:
    home = os.environ.get("TENSORFS_HOME")
    if home and os.path.isdir(home):
        return home
    for env in glob.glob("/proc/[0-9]*/environ"):
        try:
            for kv in open(env, "rb").read().split(b"\0"):
                if kv.startswith(b"TENSORFS_HOME="):
                    root = kv.split(b"=", 1)[1].decode()
                    if os.path.isdir(root):
                        return root
        except OSError:
            pass
    raise SystemExit(json.dumps({"error": "no TensorFS store"}))


def chunk_files(ckids: Sequence[str]) -> list[tuple[str, int]]:
    import tensorfs

    root, seen, out = store(), set(), []
    for ckid in ckids:
        ck = tensorfs.open(ckid, local_files_only=True)
        for sha, length in ck.chunks:
            path = os.path.join(root, "blobs", sha[:2], sha[2:4], sha)
            if sha not in seen and os.path.exists(path):
                seen.add(sha)
                out.append((path, length))
    return out


def evict(files: Sequence[tuple[str, int]]) -> None:
    os.sync()
    for path, _ in files:
        fd = os.open(path, os.O_RDONLY)
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)


def cached_share(files: Sequence[tuple[str, int]]) -> float:
    """Of the files' pages, the share the page cache holds: mincore(2) over a mapping, which faults nothing in."""
    import ctypes

    libc = ctypes.CDLL(None, use_errno=True)
    libc.mmap.restype = ctypes.c_void_p
    libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
    libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
    libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
    held = total = 0
    for path, length in files:
        if length == 0:
            continue
        fd = os.open(path, os.O_RDONLY)
        try:
            at = libc.mmap(None, length, mmap.PROT_READ, mmap.MAP_SHARED, fd, 0)
            if at in (None, ctypes.c_void_p(-1).value):
                continue
            pages = (length + 4095) // 4096
            vec = (ctypes.c_ubyte * pages)()
            if libc.mincore(at, length, vec) == 0:
                held += pages - bytes(vec).count(0)  # bit 0: resident; the others are reserved, zero
                total += pages
            libc.munmap(at, length)
        finally:
            os.close(fd)
    return round(held / total, 3) if total else 0.0


def read_grid(files: Sequence[tuple[str, int]], threads: int, request: int, direct: bool) -> dict[str, object]:
    """Every byte of `files` in `request`-sized positioned reads on `threads` threads."""
    jobs = [(p, at, min(request, length - at)) for p, length in files for at in range(0, length, request)]
    flags = os.O_RDONLY | (os.O_DIRECT if direct else 0)
    fds = {p: os.open(p, flags) for p, _ in files}
    done = [0]

    def worker(slice_: Sequence[tuple[str, int, int]]) -> int:
        buf = mmap.mmap(-1, request)
        n = 0
        for p, at, want in slice_:
            size = (want + 4095) // 4096 * 4096 if direct else want
            view = memoryview(buf)[:size]
            got = os.preadv(fds[p], [view], at)
            n += min(got, want)
        return n

    per = [jobs[i::threads] for i in range(threads)]
    began = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(threads) as pool:
        done[0] = sum(pool.map(worker, per))
    took = time.perf_counter() - began
    for fd in fds.values():
        os.close(fd)
    return {"threads": threads, "request_mib": request >> 20, "direct": direct, "gb": round(done[0] / GB, 2),
            "s": round(took, 3), "gb_s": round(done[0] / took / GB, 2)}


def disk(ckids: Sequence[str]) -> dict[str, object]:
    files = chunk_files(ckids)
    total = sum(n for _, n in files)
    # Each case reads its own files: a host that caches a guest's disk can't serve one case from another's reads.
    want = max(2 << 30, min(6 << 30, total // (len(DIRECT_GRID) + len(BUFFERED_GRID) + 1)))
    sets: list[list[tuple[str, int]]] = [[]]
    for f in files:
        if sum(n for _, n in sets[-1]) >= want:
            sets.append([])
        sets[-1].append(f)
    rows: list[dict[str, object]] = []
    for i, (threads, request) in enumerate(DIRECT_GRID):
        if i < len(sets):
            rows.append(read_grid(sets[i], threads, request, True))
    # The first case's files again: a host that caches the guest's disk reads them faster the second time.
    again = read_grid(sets[0], *DIRECT_GRID[0], True)
    for j, (threads, request) in enumerate(BUFFERED_GRID):
        k = len(DIRECT_GRID) + j
        if k < len(sets):
            evict(sets[k])
            rows.append(read_grid(sets[k], threads, request, False))
    return {"files": len(files), "gb": round(total / GB, 2), "cases": rows, "direct_again": again}


def blocks_of(ck: object, comps: Sequence[str]) -> list[tuple[str, list[tuple[str, int]]]]:
    """Each component's tensors in construction order as the Runtime's blocks hold them: a repeated block (`blocks.12.`)
    one block, every tensor outside one its own. Each part as (component/key#role, stored bytes)."""
    from tensorfs import raw

    blocks: list[tuple[str, list[tuple[str, int]]]] = []
    index: dict[str, int] = {}
    for comp in comps:
        for key in ck.components[comp].tensors:  # type: ignore[attr-defined]
            words = key.split(".")
            at = next((i for i, w in enumerate(words) if w.isdigit()), None)
            name = f"{comp}/{'.'.join(words[: at + 1]) if at is not None else key}"
            if name not in index:
                index[name] = len(blocks)
                blocks.append((comp, []))
            for role, part in raw.parts(ck, comp, key).items():  # type: ignore[arg-type]
                if part.length:
                    blocks[index[name]][1].append((f"{comp}/{key}#{role or 'value'}", part.length))
    return [b for b in blocks if b[1]]


def placed(parts: Sequence[tuple[str, int]]) -> tuple[list[tuple[str, int, int]], int]:
    """As the Runtime places a block's parts in its Region: each at the next 256 bytes."""
    out, at = [], 0
    for name, n in parts:
        out.append((name, at, n))
        at += (n + 255) // 256 * 256
    return out, at


class Card:
    def __init__(self) -> None:
        import torch
        from tensorfs.varena import _runtime as varena

        self.torch, self.varena = torch, varena
        varena.activate(0, shared=False)
        self.dev = varena.Device(0)
        self.pages = varena.Pages(0)
        self.stream = torch.cuda.Stream()
        self.s = self.stream.cuda_stream

    def room(self) -> int:
        free, _ = self.torch.cuda.mem_get_info(0)
        return int(free)

    def sources(self, ck: object, blocks: Sequence[tuple[str, list[tuple[str, int]]]], tag: str, keep: bool) -> list[tuple[object, int]]:
        out = []
        for i, (comp, parts) in enumerate(blocks):
            where, n = placed(parts)
            page = f"tfs-bound:{tag}:{i}" if keep else None
            out.append((self.varena.Source(self.pages, f"tfs-bound/{tag}/{comp}", i, n, checkpoint=ck, parts=where, page=page), n))
        return out

    def fill(self, sources: Sequence[tuple[object, int]], tag: str, limit: int) -> dict[str, object]:
        """Regions for `sources` within `limit` bytes, all filled by one `varena.fill`, then each that call left alone by its `use`."""
        chosen, total = [], 0
        for source, n in sources:
            if total + n > limit:
                break
            chosen.append((source, n))
            total += n
        self.pages.drain()
        regions = [self.dev.region(n, s, f"tfs-bound/{tag}") for s, n in chosen]
        self.torch.cuda.synchronize()
        began = time.perf_counter()
        uses = self.varena.fill(regions, [s for s, _ in chosen], self.s)
        self.stream.synchronize()
        together = time.perf_counter() - began
        alone = [r.use(self.s) for r, u in zip(regions, uses) if u is None]
        self.stream.synchronize()
        took = time.perf_counter() - began
        rows = self.pages.drain()
        by: dict[str, list[float]] = {}
        for _set, _block, nbytes, origin, _ahead, ns, _at in rows:
            row = by.setdefault(origin, [0, 0, 0])
            row[0] += nbytes
            row[1] += ns
            row[2] += 1
        out = {"blocks": len(chosen), "gb": round(total / GB, 2), "s": round(took, 3), "gb_s": round(total / took / GB, 2),
               "together_s": round(together, 3), "alone": len(alone),
               "rows": {k: {"gb": round(v[0] / GB, 2), "read_s": round(v[1] / 1e9, 3), "n": int(v[2])} for k, v in by.items()}}
        del uses, alone, regions
        gc.collect()
        self.torch.cuda.synchronize()
        return out


def fill(ckid: str, comps: Sequence[str]) -> dict[str, object]:
    import tensorfs

    ck = tensorfs.open(ckid, local_files_only=True)
    files = chunk_files([ckid])
    blocks = blocks_of(ck, comps)
    card = Card()
    limit = card.room() - (2 << 30)
    evict(files)
    report: dict[str, object] = {"checkpoint": ck.id, "components": list(comps), "blocks": len(blocks),
                                 "cached_before_cold": cached_share(files)}
    report["cold"] = card.fill(card.sources(ck, blocks, "cold", False), "cold", limit)
    report["cached_after_cold"] = cached_share(files)
    report["warm"] = card.fill(card.sources(ck, blocks, "warm", False), "warm", limit)
    return report


def paged(ckid: str, comp: str) -> dict[str, object]:
    import tensorfs

    ck = tensorfs.open(ckid, local_files_only=True)
    files = chunk_files([ckid])
    blocks = blocks_of(ck, [comp])
    card = Card()
    torch = card.torch
    report: dict[str, object] = {"checkpoint": ck.id, "component": comp, "blocks": len(blocks)}
    # Each block's stored bytes read whole into pinned RAM by its Source (the store read a page is made by).
    evict(files)
    biggest = max(placed(p)[1] for _, p in blocks)
    buf = torch.empty(biggest, dtype=torch.uint8, pin_memory=True)
    sources = card.sources(ck, blocks, "host", False)
    total, began = 0, time.perf_counter()
    for source, n in sources:
        source.read_host(buf[:n].numpy(), stored=True)  # type: ignore[attr-defined]
        total += n
    took = time.perf_counter() - began
    report["read_host"] = {"gb": round(total / GB, 2), "s": round(took, 3), "gb_s": round(total / took / GB, 2)}
    del buf, sources
    # The tier's own read ahead into pinned pages, from an empty page cache, within its room.
    evict(files)
    kept = card.sources(ck, blocks, "kept", True)
    began = time.perf_counter()
    card.pages.read_ahead([s for s, _ in kept])
    last, still = -1, 0.0
    while still < 3.0:
        time.sleep(0.05)
        st = card.pages.stats()
        now = int(st["read_ahead_bytes"])
        if now != last:
            last, still, at = now, 0.0, time.perf_counter()
        else:
            still += 0.05
        if now >= sum(n for _, n in kept) or st["read_ahead_refused"]:
            at = time.perf_counter()
            break
    took = at - began
    st = card.pages.stats()
    report["read_ahead"] = {"gb": round(last / GB, 2), "pages": int(st["read_ahead"]), "s": round(took, 3),
                            "gb_s": round(last / took / GB, 2) if took > 0 else None, "refused": int(st["read_ahead_refused"]),
                            "host_room_gb": round(int(st["room"]) / GB, 2)}
    # Those pages onto the card: pinned RAM to the device, as far as the card holds them.
    held = [(s, n) for s, n in kept]
    limit = min(card.room() - (2 << 30), int(last))
    report["from_pages"] = card.fill(held, "kept", limit)
    return report


def ceiling() -> dict[str, object]:
    import tensorfs

    c = tensorfs.ceiling(0, store())
    return {"disk_gbps": c.disk_gbps, "disk_direct": c.disk_direct, "h2d_gbps": c.h2d_gbps, "streams": c.streams}


def main() -> None:
    mode, args = sys.argv[1], sys.argv[2:]
    import tensorfs

    out: dict[str, object] = {"mode": mode, "tensorfs": tensorfs.__version__, "from": os.path.dirname(tensorfs.__file__),
                              "env": {k: v for k, v in os.environ.items() if k.startswith("TFS_X_")}}
    try:
        if mode == "ceiling":
            out.update(ceiling())
        elif mode == "evict":
            files = chunk_files(args)
            evict(files)
            out.update({"files": len(files), "gb": round(sum(n for _, n in files) / GB, 2), "cached": cached_share(files)})
        elif mode == "disk":
            out.update(disk(args))
        elif mode == "fill":
            out.update(fill(args[0], args[1].split(",")))
        elif mode == "paged":
            out.update(paged(args[0], args[1]))
        else:
            out["error"] = f"no mode {mode}"
    except Exception as error:  # noqa: BLE001
        import traceback

        out["error"] = f"{type(error).__name__}: {error}"[:600]
        out["trace"] = traceback.format_exc()[-1500:]
    print(json.dumps(out))


if __name__ == "__main__":
    main()
