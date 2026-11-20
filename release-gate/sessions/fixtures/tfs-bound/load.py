"""tfs-bound's pod side (sessions/tfs-bound.sh), run as root over `cozy machine ssh` with an executor environment's Python:
its torch, and the TensorFS the machine runs. A package can't reach a varena Source (the contract refuses `import tensorfs`),
so this follows io-fault.sh; the same models also run through `cozy run`.

    python load.py <mode> <args>   (TENSORFS_HOME: the machine's store)

  ceiling                TensorFS's own ceilings: disk GB/s of the store's volume, pinned H2D GB/s to GPU 0
  holes <ck> <comps>     the holes path and its bytes: 4 MiB of every fourth chunk cached, memory held to the admission's edge,
                         the load, and each block on the card against a plain read through the cache
  ceiling-empty          `tensorfs.ceiling(device=0)` from a shell with no TENSORFS_HOME: a home of a few small files
  evict <ck>...          the checkpoints' chunk files out of the page cache
  disk <ck>...           the disk itself, no TensorFS: the checkpoints' chunk files read O_DIRECT on a grid of threads and
                         request sizes (each on files no other read touched), then buffered from an empty page cache
  fill <ck> <comps> [reserve GiB] [alone]
                         a whole-resident load the way the Runtime binds one (cr-318): one Region a block, every block's
                         Source over its stored parts, all filled by one `varena.fill`; from disk (evicted), then from RAM.
                         `alone`: then each Region by its own `use`, in order (a fill ahead of demand, a demand fill)
  policy <ck> <comps>    where a load's bytes end up: the page cache filled to the container's number with other bytes,
                         then the load from disk and the same load again
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
    # A run downloads only what it reads (Qwen-Image 2 skips its text encoder's vision tower): a block with a part
    # in an absent chunk is left out, as a fill of it would fail.
    chunks = ck.chunks  # type: ignore[attr-defined]
    absent = {sha for sha, _ in ck.missing(list(comps))}  # type: ignore[attr-defined]
    out_of_reach: set[str] = set()

    def reaches(part: object) -> bool:
        at, end, i = 0, part.offset + part.length, part.chunk  # type: ignore[attr-defined]
        while i < len(chunks) and at < end:
            if chunks[i][0] in absent:
                return False
            at, i = at + chunks[i][1], i + 1
        return True

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
                    if not reaches(part):
                        out_of_reach.add(name)
    names = list(index)
    return [b for n, b in zip(names, blocks) if b[1] and n not in out_of_reach]


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

        from tensorfs.varena import torch as vt

        self.torch, self.varena = torch, varena
        # As an executor starts (cozy_runtime executor.py): varena's Torch allocator, the device, then Torch's CUDA.
        varena.bind_torch_allocator()
        varena.activate(0, shared=False)
        vt.initialize(0)
        self.dev = varena.Device(0)
        self.pages = varena.Pages(0)
        self.stream = torch.cuda.Stream()
        self.s = self.stream.cuda_stream

    def room(self) -> int:
        free, _ = self.torch.cuda.mem_get_info(0)
        return int(free)

    def sources(self, ck: object, blocks: Sequence[tuple[str, list[tuple[str, int]]]], tag: str, keep: bool) -> list[tuple[object, int]]:
        out: list[tuple[object, int]] = []
        for i, (comp, parts) in enumerate(blocks):
            where, n = placed(parts)
            page = f"tfs-bound:{tag}:{i}" if keep else None
            out.append((self.varena.Source(self.pages, f"tfs-bound/{tag}/{comp}", i, n, checkpoint=ck, parts=where, page=page), n))  # type: ignore[arg-type]
        return out

    def fill(self, sources: Sequence[tuple[object, int]], tag: str, limit: int, one_by_one: bool = False) -> dict[str, object]:
        """Regions for `sources` within `limit` bytes, all filled by one `varena.fill`, then each that call left alone by its
        `use`; `one_by_one`: each by its own `use`, in order, as a fill ahead of demand or a demand fill reads one."""
        chosen, total = [], 0
        for source, n in sources:
            if total + n > limit:
                break
            chosen.append((source, n))
            total += n
        self.pages.drain()
        regions = [self.dev.region(n, s, f"tfs-bound/{tag}") for s, n in chosen]  # type: ignore[arg-type]
        self.torch.cuda.synchronize()
        began = time.perf_counter()
        uses = [None] * len(regions) if one_by_one else self.varena.fill(regions, [s for s, _ in chosen], self.s)  # type: ignore[misc]
        self.stream.synchronize()
        together = time.perf_counter() - began
        alone, refused = [], ""
        for r, u in zip(regions, uses):
            if u is None:
                try:
                    alone.append(r.use(self.s))
                except Exception as error:  # noqa: BLE001
                    refused = f"{type(error).__name__}: {error}"[:200]
                    break
        self.stream.synchronize()
        took = time.perf_counter() - began
        rows = self.pages.drain()
        self.pages.request_ended()  # its store reads become `stats()["store_reads"]`: (set, block, O_DIRECT, cached, ns)
        reads = self.pages.stats()["store_reads"]
        past, through = sum(r[2] for r in reads), sum(r[3] for r in reads)
        by: dict[str, list[float]] = {}
        for _set, _block, nbytes, origin, _ahead, ns, _at in rows:
            row = by.setdefault(origin, [0, 0, 0])
            row[0] += nbytes
            row[1] += ns
            row[2] += 1
        out = {"blocks": len(chosen), "gb": round(total / GB, 2), "s": round(took, 3), "gb_s": round(total / took / GB, 2),
               "together_s": round(together, 3), "alone": len(alone), "refused": refused,
               "o_direct_gb": round(past / GB, 2), "through_cache_gb": round(through / GB, 2),
               "rows": {k: {"gb": round(v[0] / GB, 2), "read_s": round(v[1] / 1e9, 3), "n": int(v[2])} for k, v in by.items()}}
        del uses, alone, regions
        gc.collect()
        self.torch.cuda.synchronize()
        return out


def pick(ckids: str, comps: Sequence[str]) -> object:
    """Of the comma-joined checkpoints a run read, the one holding every component in `comps`."""
    import tensorfs

    for ckid in ckids.split(","):
        ck = tensorfs.open(ckid, local_files_only=True)
        if all(c in ck.components for c in comps):
            return ck
    raise SystemExit(json.dumps({"error": f"none of {ckids} holds {list(comps)}"}))


def fill(ckids: str, comps: Sequence[str], reserve: float, one_by_one: bool) -> dict[str, object]:
    ck = pick(ckids, comps)
    files = chunk_files([ck.id])  # type: ignore[attr-defined]
    blocks = blocks_of(ck, comps)
    card = Card()
    free = card.room()
    limit = free - int(reserve * (1 << 30))
    evict(files)
    report: dict[str, object] = {"checkpoint": ck.id, "components": list(comps), "blocks": len(blocks),  # type: ignore[attr-defined]
                                 "gpu_free_gb": round(free / GB, 2), "cached_before_cold": cached_share(files)}
    report["cold"] = card.fill(card.sources(ck, blocks, "cold", False), "cold", limit)
    report["cached_after_cold"] = cached_share(files)
    report["warm"] = card.fill(card.sources(ck, blocks, "warm", False), "warm", limit)
    if one_by_one:
        evict(files)
        report["cold_alone"] = card.fill(card.sources(ck, blocks, "cold-alone", False), "cold-alone", limit, True)
        report["warm_alone"] = card.fill(card.sources(ck, blocks, "warm-alone", False), "warm-alone", limit, True)
    return report


def paged(ckids: str, comp: str, reserve: float) -> dict[str, object]:
    ck = pick(ckids, [comp])
    files = chunk_files([ck.id])  # type: ignore[attr-defined]
    blocks = blocks_of(ck, [comp])
    card = Card()
    torch = card.torch
    report: dict[str, object] = {"checkpoint": ck.id, "component": comp, "blocks": len(blocks),  # type: ignore[attr-defined]
                                 "gpu_free_gb": round(card.room() / GB, 2)}
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
    card.pages.read_ahead([s for s, _ in kept])  # type: ignore[misc]
    last, still, at = -1, 0.0, began
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
    limit = min(card.room() - int(reserve * (1 << 30)), int(last))
    report["from_pages"] = card.fill(kept, "kept", limit)
    return report


def memory() -> dict[str, int]:
    """This container's memory number (cgroup v2, else v1, else the host's) and what it charges, in bytes."""
    def read(path: str) -> str:
        try:
            return open(path).read().strip()
        except OSError:
            return ""

    stat = dict(line.split() for line in read("/sys/fs/cgroup/memory.stat").splitlines() if len(line.split()) == 2)
    limit = read("/sys/fs/cgroup/memory.max") or read("/sys/fs/cgroup/memory/memory.limit_in_bytes")
    total = int(next(line.split()[1] for line in open("/proc/meminfo") if line.startswith("MemTotal"))) << 10
    number = int(limit) if limit.isdigit() and int(limit) < total else total
    current = read("/sys/fs/cgroup/memory.current") or read("/sys/fs/cgroup/memory/memory.usage_in_bytes")
    return {"limit": number, "charged": int(current) if current.isdigit() else -1,
            "file": int(stat.get("file", stat.get("cache", -1))), "anon": int(stat.get("anon", stat.get("rss", -1)))}


def policy(ckids: str, comps: Sequence[str]) -> dict[str, object]:
    """Where a load's bytes end up (cl-1318): the page cache first filled to the container's number with other bytes
    (written and synced, so clean: the kernel can take them back), so memory nothing holds is gone. Then the load from
    disk, the share of it the cache holds after, and the same load again (a fresh executor's): from memory where the
    first read kept it, else from disk again."""
    ck = pick(ckids, comps)
    files = chunk_files([ck.id])  # type: ignore[attr-defined]
    blocks = blocks_of(ck, comps)
    card = Card()
    limit = card.room() - (2 << 30)
    m = memory()
    report: dict[str, object] = {"checkpoint": ck.id, "load_gb": round(sum(placed(p)[1] for _, p in blocks) / GB, 2),  # type: ignore[attr-defined]
                                 "number_gb": round(m["limit"] / GB, 1)}
    if m["limit"] > 128 << 30:
        return {**report, "error": f"no container number to fill ({m['limit'] / GB:.0f} GB): the case needs a pod with one"}
    filler = os.path.join(store(), "tmp", "tfs-bound-filler")
    os.makedirs(filler, exist_ok=True)
    try:
        evict(files)
        want, chunk, n = m["limit"] - max(m["charged"], 0) + (2 << 30), memoryview(os.urandom(1 << 20) * 64), 0
        began = time.perf_counter()
        while n < want:
            with open(os.path.join(filler, f"{n >> 30}"), "wb") as f:
                for _ in range(16):
                    f.write(chunk)
                f.flush()
                os.fsync(f.fileno())
            n += 1 << 30
        # Read once more, so its pages are on the kernel's active list: new pages enter the inactive one, and a load that
        # pushes out its own earlier pages would show here (the review of #4117).
        again = read_grid([(os.path.join(filler, f), 1 << 30) for f in os.listdir(filler)], 8, 4 << 20, False)
        report["filler"] = {"gb": round(n / GB, 1), "s": round(time.perf_counter() - began, 1), "read_again_gb_s": again["gb_s"]}
        report["memory_before"] = {k: round(v / GB, 2) for k, v in memory().items()}
        report["cold"] = card.fill(card.sources(ck, blocks, "policy-cold", False), "policy-cold", limit)
        report["cached_after_cold"] = cached_share(files)
        report["again"] = card.fill(card.sources(ck, blocks, "policy-again", False), "policy-again", limit)
        report["memory_after"] = {k: round(v / GB, 2) for k, v in memory().items()}
    finally:
        for name in os.listdir(filler):
            os.remove(os.path.join(filler, name))
    return report


def holes(ckids: str, comps: Sequence[str]) -> dict[str, object]:
    """The holes path (cl-1318) and its bytes: the first 4 MiB of every fourth chunk in the page cache, memory held
    until the host admission leaves less than a read's missing bytes, then the load (past the cache in its holes, the rest
    from it), and each block's bytes on the card against the same block read plainly through the cache (`Source.read_host`).
    Clean cache counts toward the admission, so all that is cached stays under one batch's missing bytes: with half of each
    chunk cached (nadja, 29 GiB) the hold took it all back, and every read went past the cache whole."""
    import numpy as np

    ck = pick(ckids, comps)
    files = chunk_files([ck.id])  # type: ignore[attr-defined]
    blocks = blocks_of(ck, comps)
    card = Card()
    limit = card.room() - (2 << 30)
    m = memory()
    report: dict[str, object] = {"checkpoint": ck.id, "number_gb": round(m["limit"] / GB, 1)}  # type: ignore[attr-defined]
    if m["limit"] > 128 << 30:
        return {**report, "error": f"no container number to press against ({m['limit'] / GB:.0f} GB): the case needs a pod with one"}
    evict(files)
    for path, _ in files[::4]:
        fd = os.open(path, os.O_RDONLY)
        try:
            os.pread(fd, 4 << 20, 0)
        finally:
            os.close(fd)
    report["cached_before"] = cached_share(files)
    # Held until what the admission leaves (the number less what is held and the headroom, the cached runs in it) is under a
    # 512 MiB batch's missing bytes. The headroom stays free, and the load's O_DIRECT reads add no cache.
    m = memory()
    headroom = max(m["limit"] // 16, 512 << 20)
    pin = max(0, m["limit"] - (m["charged"] - m["file"]) - headroom - (64 << 20))
    held = np.ones(pin, np.uint8) if pin else None
    report["held_gb"] = round(pin / GB, 2)
    report["cached_after_hold"] = cached_share(files)  # the hold may take some of the cached half back
    report["memory_before"] = {k: round(v / GB, 2) for k, v in memory().items()}
    sources = card.sources(ck, blocks, "holes", False)
    chosen, total = [], 0
    for source, n in sources:
        if total + n > limit:
            break
        chosen.append((source, n))
        total += n
    card.pages.drain()
    regions = [card.dev.region(n, s, "tfs-bound/holes") for s, n in chosen]  # type: ignore[arg-type]
    card.torch.cuda.synchronize()
    began = time.perf_counter()
    uses = card.varena.fill(regions, [s for s, _ in chosen], card.s)
    card.stream.synchronize()
    took = time.perf_counter() - began
    card.pages.request_ended()
    reads = card.pages.stats()["store_reads"]
    report["load"] = {"gb": round(total / GB, 2), "s": round(took, 3), "o_direct_gb": round(sum(r[2] for r in reads) / GB, 2),
                      "through_cache_gb": round(sum(r[3] for r in reads) / GB, 2), "alone": sum(1 for u in uses if u is None)}
    del held
    # Each block's bytes on the card against the same block read through the cache, plainly.
    wrong, checked = [], 0
    for i, ((source, n), use, region) in enumerate(zip(chosen, uses, regions)):
        u = use if use is not None else region.use(card.s)
        card.stream.synchronize()
        got = u.tensor.cpu().numpy()
        want = np.zeros(n, np.uint8)
        source.read_host(want, stored=True)  # type: ignore[attr-defined]
        checked += n
        if not np.array_equal(got[:n], want):
            wrong.append(i)
    report["bytes"] = {"blocks": len(chosen), "gb": round(checked / GB, 2), "wrong_blocks": wrong}
    return report


def ceiling_empty() -> dict[str, object]:
    """`tensorfs.ceiling(device=0)` as a shell calls it with no TENSORFS_HOME: a home of a few small files (its own
    table, a header), in a process of its own, so nothing kept answers for the volume."""
    import subprocess
    import tempfile

    with tempfile.TemporaryDirectory(dir=os.path.dirname(store())) as home:
        root = os.path.join(home, ".cache", "tensorfs")
        os.makedirs(os.path.join(root, "tmp", "ceilings"))
        for i in range(3):
            with open(os.path.join(root, f"small-{i}"), "wb") as f:
                f.write(os.urandom(4096))
        env = {k: v for k, v in os.environ.items() if k != "TENSORFS_HOME"} | {"HOME": home}
        code = "import json, tensorfs; c = tensorfs.ceiling(device=0); print(json.dumps({'disk_gbps': c.disk_gbps, 'disk_direct': c.disk_direct}))"
        done = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=env, timeout=600)
        try:
            return {"home": root, **json.loads(done.stdout.strip().splitlines()[-1])}
        except (ValueError, IndexError):
            return {"home": root, "error": done.stderr[-400:]}


def memcpy(threads: int, each: int = 256 << 20, rounds: int = 8) -> float:
    """GB/s of `threads` threads each copying its own `each` bytes, `rounds` times: the host's copy bandwidth (each byte
    read and written once), what a read from the page cache into pinned staging spends."""
    import numpy as np

    pairs = [(np.ones(each, np.uint8), np.empty(each, np.uint8)) for _ in range(threads)]

    def work(pair: tuple[object, object]) -> None:
        for _ in range(rounds):
            np.copyto(pair[1], pair[0])  # type: ignore[arg-type]

    began = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(threads) as pool:
        list(pool.map(work, pairs))
    return round(threads * each * rounds / (time.perf_counter() - began) / GB, 2)


def ceiling() -> dict[str, object]:
    import tensorfs

    c = tensorfs.ceiling(0, store())
    return {"disk_gbps": c.disk_gbps, "disk_direct": c.disk_direct, "h2d_gbps": c.h2d_gbps, "streams": c.streams,
            "cpus": len(os.sched_getaffinity(0)), "memcpy_gbps": {n: memcpy(n) for n in (1, 4, 8)}}


def main() -> None:
    import resource

    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (hard, hard))  # a grid read holds every file of a checkpoint open
    mode, args = sys.argv[1], sys.argv[2:]
    import tensorfs

    out: dict[str, object] = {"mode": mode, "tensorfs": tensorfs.__version__, "from": os.path.dirname(tensorfs.__file__),
                              "env": {k: v for k, v in os.environ.items() if k.startswith("TFS_X_")}}
    try:
        if mode == "ceiling":
            out.update(ceiling())
        elif mode == "ceiling-empty":
            out.update(ceiling_empty())
        elif mode == "evict":
            files = chunk_files(args)
            evict(files)
            out.update({"files": len(files), "gb": round(sum(n for _, n in files) / GB, 2), "cached": cached_share(files)})
        elif mode == "disk":
            out.update(disk(args))
        elif mode == "fill":
            out.update(fill(args[0], args[1].split(","), float(args[2]) if len(args) > 2 else 2.0, "alone" in args[3:]))
        elif mode == "holes":
            out.update(holes(args[0], args[1].split(",")))
        elif mode == "policy":
            out.update(policy(args[0], args[1].split(",")))
        elif mode == "paged":
            out.update(paged(args[0], args[1], float(args[2]) if len(args) > 2 else 2.0))
        else:
            out["error"] = f"no mode {mode}"
    except Exception as error:  # noqa: BLE001
        import traceback

        out["error"] = f"{type(error).__name__}: {error}"[:600]
        out["trace"] = traceback.format_exc()[-1500:]
    print(json.dumps(out))


if __name__ == "__main__":
    main()
