"""Coordinator-only GPU regression: actual capture/replay, retry and finalizer custody.

Run through the ordinary Cozy gate with the matching P4/core/Torch shim. Not a host test.
"""
import argparse
import ctypes as C
import gc
import json
import threading
import time
from contextlib import contextmanager

import torch
from streams import owned_stream
from tensorfs.varena import _runtime as varena
from tensorfs.varena import api
from tensorfs.varena import torch as vt


class Graph:
    """Actual empty CUDA graph; no unprepared Torch private-pool fixture allocations."""
    def __init__(self, stream):
        self.cuda = C.CDLL("libcuda.so.1")
        self.stream = C.c_void_p(stream.cuda_stream)
        self.graph, self.executable = C.c_void_p(), C.c_void_p()
        for name, args in {
            "cuStreamBeginCapture": [C.c_void_p, C.c_int],
            "cuStreamEndCapture": [C.c_void_p, C.POINTER(C.c_void_p)],
            "cuGraphInstantiateWithFlags": [C.POINTER(C.c_void_p), C.c_void_p, C.c_uint64],
            "cuGraphLaunch": [C.c_void_p, C.c_void_p],
            "cuGraphDestroy": [C.c_void_p], "cuGraphExecDestroy": [C.c_void_p],
            "cuStreamSynchronize": [C.c_void_p],
        }.items():
            getattr(self.cuda, name).argtypes = args
            getattr(self.cuda, name).restype = C.c_int

    def close(self):
        assert self.cuda.cuStreamSynchronize(self.stream) == 0
        if self.executable:
            assert self.cuda.cuGraphExecDestroy(self.executable) == 0
            self.executable = C.c_void_p()
        if self.graph:
            assert self.cuda.cuGraphDestroy(self.graph) == 0
            self.graph = C.c_void_p()

    def begin(self):
        assert self.cuda.cuStreamBeginCapture(self.stream, 0) == 0

    def end(self):
        assert self.cuda.cuStreamEndCapture(self.stream, C.byref(self.graph)) == 0

    def prepare(self):
        assert self.cuda.cuGraphInstantiateWithFlags(C.byref(self.executable), self.graph, 0) == 0

    def replay(self):
        assert self.cuda.cuGraphLaunch(self.executable, self.stream) == 0


def drain():
    # A bounded proof harness, not a product timeout or assumption that work completed.
    deadline = time.monotonic() + 30
    while api.retirement_stats()["queued"]:
        assert time.monotonic() < deadline, api.retirement_stats()
        time.sleep(0.01)
    return api.retirement_stats()


@contextmanager
def blocked(other, graph, stream, replay):
    if replay:
        with varena.replay(0):
            graph.replay()
            yield
    else:
        graph.close()
        with other.capture(), torch.cuda.stream(stream):
            graph.begin()
            try:
                yield
            finally:
                graph.end()
        graph.prepare()


def refused(call):
    try:
        call()
    except api.Capturing:
        return
    raise AssertionError("checked close must preserve its handle during capture/replay")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", required=True, help="matching libvarena_mixed_client.so")
    args = parser.parse_args()
    rust = C.CDLL(args.rust)
    vt.install(0)
    torch.empty(1, device="cuda").sum().item()
    torch.cuda.current_stream().synchronize()
    other = varena.Arena(0)
    rust.mixed_rust_retry_prepare.argtypes = [C.c_uint64]
    with owned_stream(torch, vt) as stream:
        run_retirement(rust, other, stream)


def run_retirement(rust, other, stream):
    graph = Graph(stream)
    receipts = []
    for replay in (False, True):
        drain()
        gpu = api.open()
        baseline = gpu.stats()
        stream.synchronize()
        driver_before = torch.cuda.mem_get_info()
        region = gpu.region(bytes(1 << 20))
        used = gpu.use([region], stream.cuda_stream)
        scratch = gpu.alloc(2 << 20, stream.cuda_stream)
        original = (gpu._handle, region._handle, used._handle, scratch._handle)
        assert rust.mixed_rust_retry_prepare(stream.cuda_stream) == 1
        with blocked(other, graph, stream, replay):
            assert rust.mixed_rust_retry_refused() == 1
            for owner in (scratch, used, region, gpu):
                refused(owner.close)
            assert original == (gpu._handle, region._handle, used._handle, scratch._handle)
        # The exact same owners/pointers can be used and then closed after refusal.
        assert rust.mixed_rust_retry_finish() == 1
        assert used.ptr() and scratch.ptr
        scratch.close()
        used.close()
        region.close()
        gpu.close()
        drain()
        with api.open() as inspect:
            inspect.trim()
            assert inspect.stats()["weights_resident"] == baseline["weights_resident"]
        drain()

        gpu = api.open()
        region = gpu.region(bytes(1 << 20))
        used = gpu.use([region], stream.cuda_stream)
        scratch = gpu.alloc(2 << 20, stream.cuda_stream)
        before = api.retirement_stats()
        with blocked(other, graph, stream, replay):
            del scratch, used, region, gpu
            gc.collect()
            assert api.retirement_stats()["queued"] > before["queued"]
        stream.synchronize()
        retired = drain()
        assert retired["quarantined"] == before["quarantined"]
        with api.open() as inspect:
            inspect.trim()
            final = inspect.stats()
            assert final["weights_resident"] == baseline["weights_resident"]
            assert final["working_live"] == baseline["working_live"]
        drain()
        stream.synchronize()
        driver_after = torch.cuda.mem_get_info()
        receipts.append({"replay": replay, "checked_retry_same_handles": True, "retired": retired, "final": final,
                         "driver_free_total_before": driver_before, "driver_free_total_after": driver_after})

    graph.close()
    gpu = api.open()
    cache = gpu.cache(64 << 20)
    page = cache.info()["granularity"]
    cache.commit(0, page)
    with torch.cuda.stream(stream):
        torch.cuda._sleep(1_000_000_000)
    cache.release(0, page, stream.cuda_stream)
    assert cache.info()["pending"] == page, "fixture did not establish a pending release"
    before = api.retirement_stats()
    with varena.replay(0):
        del cache
        gc.collect()
        assert api.retirement_stats()["queued"] > before["queued"]
    stream.synchronize()
    assert drain()["quarantined"] == before["quarantined"]
    assert gpu.stats()["caches"] == 0

    # A real, already-running Python callback must retain its release/read borrow
    # after the caller transfers ownership, and new callers must be excluded.
    entered, resume, finished = threading.Event(), threading.Event(), threading.Event()
    callback_errors = []

    def shrink(view, requested):
        entered.set()
        try:
            assert resume.wait(30), "callback release gate was not opened"
            assert view.info()["live"] == page
            try:
                api._call("cache_commit", view._handle, page, page)
            except api.Invalid:
                pass
            else:
                raise AssertionError("queued callback was allowed to commit new pages")
            view.release(0, page, stream.cuda_stream)
            finished.set()
            return True
        except BaseException as error:
            callback_errors.append(repr(error))
            finished.set()
            raise

    cache = gpu.cache(64 << 20, shrink=shrink)
    cache.commit(0, page)
    request = varena.request_cache_shrink(0, page)
    assert request is not None and entered.wait(30)
    handle = cache._handle
    before = api.retirement_stats()
    cache.defer_close()
    assert cache._owner.facts()["closing"]
    try:
        api.CacheView(handle, gpu).info()
    except api.Invalid:
        pass
    else:
        raise AssertionError("ordinary caller borrowed a transferred Cache")
    resume.set()
    assert finished.wait(30) and not callback_errors, callback_errors
    stream.synchronize()
    assert drain()["quarantined"] == before["quarantined"]
    assert cache._owner.facts()["retired"]
    assert gpu.stats()["caches"] == 0
    cache.close()

    # Deliberate live-page abandonment is a distinct observable quarantine, not a pass
    # claiming every byte was freed. Run this last in its own short-lived process.
    cache = gpu.cache(64 << 20)
    cache.commit(0, page)
    before = api.retirement_stats()
    del cache
    gc.collect()
    after = drain()
    assert after["quarantined"] == before["quarantined"] + 1
    assert after["abandoned_cache_bytes"] == before["abandoned_cache_bytes"] + page
    gpu.close()
    drain()
    print(json.dumps({"gpu": True, "lifetimes": receipts, "callback_release_during_retirement": True,
                      "live_cache_quarantine": after, "driver_baseline_qualified": False,
                      "driver_baseline_gate": "raw driver samples are retained; exact physical baseline still requires a controlled warmed coordinator cell"}))


if __name__ == "__main__":
    main()
