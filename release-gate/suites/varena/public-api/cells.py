"""Real cells of the public API, each a fresh process under a cap: any failed assert fails the run.
  sizing --engine SO     a C engine sizes its work with room and split
  priority high|normal   who goes first: a high Region after the KV is asked, a normal one before it
  priority peer          a second process's ask reaches a high Region only after the KV answers
"""
import argparse
import ctypes as C
import json
import subprocess
import sys
from collections.abc import Callable
from typing import TypedDict

import tensorfs.varena.torch as vt
import torch
from cuda_driver import Stream, primary
from tensorfs import varena

MIB = 1 << 20
CAP = 1024 * MIB
REGION = 128 * MIB
KV = 384 * MIB
OVER = 64 * MIB


class Args(argparse.Namespace):
    run: Callable[["Args"], None]
    seed: int
    engine: str  # sizing
    mode: str  # priority: high, normal or peer


# sized_engine.c's report. A row's `pass` is a keyword, so its rows are spelled the functional way.
SizedRow = TypedDict("SizedRow", {"key": str, "pass": int, "level": int, "counted": int, "peak": int, "budget": int,
                                  "pieces": int, "measured": bool, "bytes": int, "estimate": int})


class Sized(TypedDict):
    rows: list[SizedRow]
    job: int
    capacity: int
    peak: int
    mapped: int
    resident: int
    driver_ooms: int


class SceneRow(TypedDict):
    step: str
    resident: int
    kv_live: int
    asks: int


def cap(managed: int) -> None:
    """Leave this much for our own bytes, the core's slack on top."""
    vt.install(0)
    free, total = torch.cuda.mem_get_info(0)
    owned = vt.stats(0)["mapped_bytes"]
    vt.set_limit(0, 1 << 50)  # Any cap makes the core read the driver's reservation.
    reserved = vt.stats(0)["driver_reservation"]
    vt.set_limit(0, managed + max(total - free - owned, 0) + 128 * MIB + reserved)


def sizing(args: Args) -> None:
    cap(CAP)
    engine = C.CDLL(args.engine)
    engine.sized_engine.argtypes = [C.c_uint64, C.c_char_p, C.c_size_t]
    report = C.create_string_buffer(1 << 16)
    assert engine.sized_engine(args.seed, report, len(report)) == 0, report.value
    result: Sized = json.loads(report.value)
    rows = {(row["key"], row["pass"]): row for row in result["rows"]}
    job, capacity = result["job"], result["capacity"]
    assert abs(capacity - CAP) < 256 * MIB, result
    for row in rows.values():
        assert row["counted"] == 1 and row["peak"] == job >> row["level"] and row["budget"] < capacity, row
        assert row["pieces"] > 1, row  # The whole job never fits beside the weights.
    # What was estimated right: the same level, now on its own measure.
    first, second = rows["honest", 0], rows["honest", 1]
    assert not first["measured"] and first["bytes"] == first["estimate"], first
    assert second["level"] == first["level"] and second["measured"] and second["bytes"] == first["peak"], second
    # What was overestimated: the measure lets the second pass run coarser, and the third on its own measure.
    first, second, third = rows["over", 0], rows["over", 1], rows["over", 2]
    assert not first["measured"] and first["bytes"] == first["estimate"], first
    assert second["level"] < first["level"] and not second["measured"] and second["bytes"] < second["estimate"], second
    assert second["bytes"] == second["estimate"] * first["peak"] // first["estimate"], second
    assert third["level"] == second["level"] and third["measured"] and third["bytes"] == second["peak"], third
    # The cap held, on our own bytes, and no driver refusal taught us that.
    assert result["peak"] <= capacity and result["mapped"] + result["resident"] <= capacity, result
    assert result["driver_ooms"] == 0, result
    print(json.dumps({"sizing": "pass", **result}))


class Scene:
    """Two Regions and a KV Cache, all resident. `shrink` drops KV pages unless `decline` is set."""

    asked: list[int]  # each `requested` shrink was asked for

    def __init__(self, gpu: varena.Gpu, stream: Stream, seed: int) -> None:
        self.gpu, self.stream, self.asked, self.decline, self.taken = gpu, stream, [], False, 0
        self.fill = seed % 200 + 1
        self.keep = bytes([self.fill]) * REGION
        self.high = gpu.region(self.keep)
        self.normal = gpu.region(bytes([self.fill + 1]) * REGION)
        self.cache = gpu.cache(KV, shrink=self.shrink)
        self.base, self.page = self.cache.info()["base"], self.cache.info()["granularity"]
        self.cache.commit(0, KV)
        stream.fill(self.base, self.fill, KV)
        stream.finish()
        for region in (self.high, self.normal):
            with gpu.use([region], stream.handle) as used:
                used.wait_ready()
                stream.finish()
        assert self.resident() == 2 * REGION

    def shrink(self, cache: varena.CacheView, requested: int) -> bool:
        self.asked.append(requested)
        if self.decline:
            return False
        bytes_ = min(-(-requested // self.page) * self.page, KV - self.taken)
        cache.release(self.taken, bytes_, self.stream.handle)
        self.taken += bytes_
        return True

    def resident(self) -> int:
        return self.gpu.stats()["weights_resident"]

    def live(self) -> int:
        return self.cache.info()["live"]

    def press(self) -> varena.Allocation:
        """Ask for OVER more than is free: something must go."""
        return self.gpu.alloc(self.gpu.room()["free"] + OVER, self.stream.handle)

    def row(self, step: str) -> SceneRow:
        return {"step": step, "resident": self.resident(), "kv_live": self.live(), "asks": len(self.asked)}

    def check_kv(self) -> None:
        assert self.stream.read(self.base + KV - self.page, self.page) == bytes([self.fill]) * self.page

    def check_high(self) -> None:
        with self.gpu.use([self.high], self.stream.handle) as used:
            assert self.stream.read(used.ptr(), REGION) == self.keep

    def close(self, held: list[varena.Allocation]) -> None:
        for allocation in held:
            allocation.close()
        self.check_high()
        # What the owner dropped comes back whole: release all, then recommit what it needs.
        if self.taken:
            self.cache.commit(0, self.page)
            self.stream.fill(self.base, 9, self.page)
            assert self.stream.read(self.base, self.page) == bytes([9]) * self.page
            self.cache.release(0, self.page, self.stream.handle)
        self.cache.release(self.taken, KV - self.taken, self.stream.handle)
        self.stream.finish()
        self.cache.close()
        self.normal.close()
        self.high.close()


def priority(args: Args) -> None:
    high = args.mode != "normal"
    cuda = C.CDLL("libcuda.so.1")
    if args.mode == "peer":
        return peer(args, cuda)
    cap(CAP)
    with primary(cuda), Stream(cuda) as stream, varena.open() as gpu:
        scene = Scene(gpu, stream, args.seed)
        if high:
            scene.high.set_priority("high")
        rows, held = [scene.row("start")], [scene.press()]
        rows.append(scene.row("one region's worth of pressure"))
        assert rows[-1]["resident"] == REGION and not scene.asked, rows  # A normal Region goes first.
        held.append(scene.press())
        rows.append(scene.row("the next"))
        if high:
            # The KV shrinks and the high Region stays, every byte of what's left intact.
            assert rows[-1]["resident"] == REGION and scene.asked and rows[-1]["kv_live"] < KV, rows
            scene.check_kv()
            scene.decline = True
            kept, asks = scene.live(), len(scene.asked)
            held.append(scene.press())
            rows.append(scene.row("the KV declines"))
            assert len(scene.asked) > asks, rows  # Only after it was asked,
            assert rows[-1]["resident"] == 0 and scene.live() == kept, rows  # does the high Region go.
        else:
            # Today's order: every weight before any Cache.
            assert rows[-1]["resident"] == 0 and not scene.asked and rows[-1]["kv_live"] == KV, rows
        scene.close(held)
        print(json.dumps({"priority": args.mode, "rows": rows, "room": gpu.room()}))


def peer(args: Args, cuda: C.CDLL) -> None:
    """An idle process holds a high Region and a KV that declines; another takes the card's free memory."""
    with primary(cuda), Stream(cuda) as stream, varena.open() as gpu:
        scene = Scene(gpu, stream, args.seed)
        scene.high.set_priority("high")
        scene.decline = True
        scene.normal.close()
        ask = subprocess.run([sys.executable, __file__, "take"], capture_output=True, text=True, timeout=120)
        assert ask.returncode == 0, f"take exited {ask.returncode}: {ask.stdout}{ask.stderr}"
        row = scene.row("a peer took more than was free")
        assert scene.asked and row["resident"] == 0 and row["kv_live"] == KV, row
        scene.close([])
        print(json.dumps({"priority": "peer", "row": row, "taken": json.loads(ask.stdout)}))


def take(_: Args) -> None:
    cuda = C.CDLL("libcuda.so.1")
    with primary(cuda), Stream(cuda) as stream, varena.open() as gpu:
        room = gpu.room()
        with gpu.alloc(room["free"] + OVER, stream.handle):
            print(json.dumps({"free": room["free"], "over": OVER}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(required=True)
    command = commands.add_parser("sizing")
    command.add_argument("--engine", required=True)
    command.set_defaults(run=sizing)
    command = commands.add_parser("priority")
    command.add_argument("mode", choices=["high", "normal", "peer"])
    command.set_defaults(run=priority)
    commands.add_parser("take").set_defaults(run=take)
    parser.add_argument("--seed", type=int, default=138)
    args = parser.parse_args(namespace=Args())
    args.run(args)
