"""Coordinator-only real CUDA clients; each case requires a fresh owned child.

Uses real native Cache pages and actual Torch allocations. Fixture byte sizes are
explicit test geometry, never a product policy or a substitute for the full SDXL gate.
"""
from __future__ import annotations

import argparse
from contextlib import nullcontext
import ctypes
import json
from pathlib import Path
import threading
import time

import torch
from tensorfs import _ext
from tensorfs.varena import _runtime, api
from tensorfs.varena import torch as vt

MIB = 1 << 20


class ResizeResult(ctypes.Structure):
    _fields_ = [(name, ctypes.c_int) for name in (
        "gil_before", "gil_after", "failed", "expected_exception", "callback_marker"
    )] + [("error", ctypes.c_char * 2048), ("exception_type", ctypes.c_char * 256)]


def until(predicate, label, seconds=5):
    deadline = time.monotonic() + seconds
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError(label)
        time.sleep(0.001)


def run(args):
    device = args.device
    vt.install(device)
    gpu = api.open(device)
    stream = torch.cuda.Stream(device=device)
    requested = 32 * MIB
    page_bytes = 16 * MIB
    caches = [_ext._VarenaCache(device, page_bytes) for _ in range(2)]
    for cache in caches:
        cache.commit(0, page_bytes)
    owners, callbacks, events = [], [], []
    entered = threading.Event()
    release = threading.Event()
    done = threading.Event()
    marker = ctypes.c_int(0)
    helper_errors = []

    def callback(index):
        def shrink(wanted):
            marker.value += 1
            events.append({"owner": index, "event": "entered", "wanted": wanted,
                           "at_ns": time.monotonic_ns()})
            blocked = (args.case == "progress" and index == 1) or (
                args.case in ("ordinary", "external-callback", "other-thread") and index == 0
            )
            if blocked:
                entered.set()
                assert release.wait(8), "fixture callback release deadline"
            caches[index].release(0, page_bytes, stream.cuda_stream)
            owners[index].suspend()
            events.append({"owner": index, "event": "returned", "at_ns": time.monotonic_ns()})
        return shrink

    for index in range(2):
        callbacks.append(callback(index))
        owners.append(_runtime.CacheOwner(device, callbacks[-1]))

    def release_second():
        try:
            assert entered.wait(5), "second owner did not enter"
            assert caches[0].stats()["mapped_bytes"] == 0
            assert page_bytes < requested
            assert owners[1].facts()["running"]
            # The second callback remains active while this real allocation is
            # pending after an insufficient release. Scheduling is not a witness
            # of the exact point at which an older caller rechecked eligibility.
            premature = done.wait(0.15)
            events.append({"event": "held_second_owner", "premature_return": premature,
                           "first_released": page_bytes, "requested": requested,
                           "working": vt.stats(device), "at_ns": time.monotonic_ns()})
            assert not premature, "allocation returned before required second owner release"
        except BaseException as error:
            helper_errors.append(repr(error))
        finally:
            release.set()

    token = output = helper = None
    report = {"case": args.case, "torch": torch.__version__, "torch_git": torch.version.git_version,
              "requested": requested, "cache_page_bytes": page_bytes}
    with torch.inference_mode(), torch.cuda.stream(stream):
        live = torch.full((8 * MIB,), 37, dtype=torch.uint8, device=device)
        stream.synchronize()
        vt.trim(device)
        before = vt.stats(device)
        limit = before["mapped_bytes"] + before["cache_mapped_bytes"] + gpu.stats()["weights_resident"]
        if args.case == "no-pressure":
            limit += requested
        vt.set_limit(device, limit)
        key = "cold-recovery:" + args.case + ":" + str(time.monotonic_ns())
        token = (vt.phase_enter(device, key) if args.case == "ordinary"
                 else vt._phase_enter_cold(device, key))
        report.update(before=before, managed_limit=limit, key=key)
        if args.case == "progress":
            helper = threading.Thread(target=release_second)
            helper.start()
        start = time.monotonic()
        error = None
        try:
            if args.case == "timeout":
                assert args.gil_helper is not None
                library = ctypes.PyDLL(str(args.gil_helper))
                library.resize_fixture_abi.restype = ctypes.c_int
                assert library.resize_fixture_abi() == 2
                resize = library.resize_holding_gil
                resize.argtypes = [ctypes.py_object, ctypes.c_uint64, ctypes.py_object,
                                   ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ResizeResult)]
                resize.restype = ctypes.c_int
                storage = torch.UntypedStorage(0, device=f"cuda:{device}")
                result = ResizeResult()
                code = resize(storage, requested, torch.OutOfMemoryError,
                              ctypes.byref(marker), ctypes.byref(result))
                elapsed = time.monotonic() - start
                assert code == 0 and result.gil_before == result.gil_after == 1
                assert result.failed and result.expected_exception and result.callback_marker == 0
                assert 1.8 <= elapsed < 5, "existing two-second native wait budget"
                assert storage.nbytes() == 0
                report["gil_control"] = {"gil_before": result.gil_before, "gil_after": result.gil_after,
                                         "callback_marker_before_return": result.callback_marker,
                                         "elapsed_s": elapsed, "error": bytes(result.error).decode()}
                error = "OutOfMemoryError"
            elif args.case == "other-thread":
                result = []

                def allocate_other():
                    try:
                        with torch.cuda.device(device), torch.cuda.stream(stream):
                            result.append(torch.empty(requested, dtype=torch.uint8, device=device))
                    except BaseException as caught:
                        result.append(caught)

                thread = threading.Thread(target=allocate_other)
                thread.start()
                thread.join(5)
                assert not thread.is_alive()
                assert isinstance(result[0], torch.OutOfMemoryError)
                error = type(result[0]).__name__
            else:
                scope = vt.external_callback(device) if args.case == "external-callback" else nullcontext()
                with scope:
                    output = torch.empty(requested, dtype=torch.uint8, device=device)
        except torch.OutOfMemoryError:
            error = "OutOfMemoryError"
        finally:
            done.set()
            if helper is not None:
                helper.join(8)
                assert not helper.is_alive()
            elapsed = time.monotonic() - start
            try:
                if args.case in ("ordinary", "external-callback", "other-thread"):
                    report["nonwaiting_seconds"] = elapsed
                    assert error == "OutOfMemoryError" and elapsed < 1.5
                    assert not release.is_set(), "callback must remain unavailable until return"
            finally:
                release.set()

        assert not helper_errors, helper_errors
        if args.case in ("progress", "no-pressure"):
            assert error is None and output is not None
            output.fill_(19)
            stream.synchronize()
            assert bool((output.cpu() == 19).all())
        else:
            assert error == "OutOfMemoryError"
        stream.synchronize()
        observed = vt.phase_leave(device, token, completed=error is None)
        token = None
        if args.case == "progress":
            assert not observed["published"] and observed["reason"] == "cache_recovery"
            until(lambda: all(cache.stats()["mapped_bytes"] == 0 for cache in caches), "physical cache release")
            until(lambda: all(owner.facts()["returned"] == 1 and not owner.facts()["running"] for owner in owners), "owner return")
        elif args.case == "no-pressure":
            assert observed["published"]
            assert all(owner.facts()["callbacks"] == 0 for owner in owners)
            assert all(cache.stats()["mapped_bytes"] == page_bytes for cache in caches)
        elif args.case == "timeout":
            assert not observed["published"] and observed["reason"] == "cache_recovery"
            until(lambda: all(cache.stats()["mapped_bytes"] == 0 for cache in caches), "late callback custody")
        assert bool((live.cpu() == 37).all()), "unrelated live allocation changed"
        report.update(error=error, elapsed_s=elapsed, observation=observed,
                      owners=[owner.facts() for owner in owners], after=vt.stats(device), events=events)
        vt.set_limit(device, None)
        del output
        vt.trim(device)
        if args.case == "progress":
            # A later natural call can publish a clean sample; no body is rerun.
            clean_token = vt._phase_enter_cold(device, key)
            clean = torch.empty(requested, dtype=torch.uint8, device=device)
            clean.fill_(19)
            stream.synchronize()
            assert bool((clean.cpu() == 19).all())
            clean_sample = vt.phase_leave(device, clean_token, completed=True)
            assert clean_sample["published"]
            report["later_clean_sample"] = clean_sample
            del clean
        for owner in owners:
            owner.close()
        for cache in caches:
            if cache.stats()["live_bytes"]:
                cache.release(0, page_bytes, stream.cuda_stream)
        stream.synchronize()
        for cache in caches:
            cache.close()
        del live
    report["passed"] = True
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--case", required=True, choices=("progress", "timeout", "ordinary", "external-callback", "other-thread", "no-pressure"))
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--gil-helper", type=Path)
    args = parser.parse_args()
    print(json.dumps(run(args), indent=2))
