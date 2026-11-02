"""Real Python Arena root disposal; no pre-request masks the destructor-only arm."""

import ctypes as C
import gc
import hashlib
import json
import os
import time
import weakref
from importlib.metadata import version
from pathlib import Path


def exercise(payload, ctx):
    import tensorfs
    import torch
    from cozy_runtime.internal import cooperation, int8_linear
    from cozy_runtime.internal.residency import _REQUEST
    from tensorfs import _ext
    from tensorfs.varena import _runtime as varena

    from . import Report

    request = _REQUEST.get()
    assert request is not None and request.id == ctx.request_id
    owner = request.memory.cooperation
    assert (
        owner is not None
        and owner.facts()["active"]
        and owner.config.uuid == payload.gpu_uuid
    )
    actual = {
        "runtime_version": version("cozy-runtime"),
        "runtime_owner_sha256": hashlib.sha256(
            Path(cooperation.__file__).read_bytes()
        ).hexdigest(),
        "tensorfs_version": version("tensorfs"),
        "extension_sha256": hashlib.sha256(
            Path(_ext.__file__).read_bytes()
        ).hexdigest(),
        "shim_sha256": hashlib.sha256(
            (Path(tensorfs.__file__).parent / "_varena_torch.so").read_bytes()
        ).hexdigest(),
    }
    assert all(actual[key] == getattr(payload, key) for key in actual), actual
    assert (
        str(torch.cuda.get_device_properties(0).uuid).removeprefix("GPU-").lower()
        == payload.gpu_uuid.removeprefix("GPU-").lower()
    )
    int8_linear.native(
        torch, 0
    )  # Join the ordinary executor's existing readiness work.
    native = owner._peer
    native_row = native.snapshot()
    original = next(
        row for row in native_row["rows"] if row["id"] == native_row["owner"]
    )
    extra = varena.CooperationOwner(
        0,
        payload.gpu_uuid,
        owner.config.directory,
        shared=owner.config.shared,
        priority=owner.config.priority,
    )
    callback_calls = []

    def decline(wanted):
        callback_calls.append(wanted)
        return False

    cache_owner = varena.CacheOwner(0, decline)
    stream = torch.cuda.Stream()
    arena = region = used = escaped = plan = None
    driver = None
    captured = False
    raw_stream = C.c_void_p()
    graph = C.c_void_p()

    def until(operation):
        end = time.monotonic() + 15
        while True:
            try:
                value = operation()
                if value is not None:
                    return value
            except varena.CooperationDeferredError:
                pass
            assert time.monotonic() < end, "real Python Arena retirement did not settle"
            time.sleep(0.005)

    def row():
        table = native.snapshot()
        return next(value for value in table["rows"] if value["id"] == table["owner"])

    def weights():
        return sum(row()["physical"]["weights"].values())

    def publish():
        before = owner.facts()["published"]
        native.wake()
        until(lambda: True if owner.facts()["published"] > before else None)
        return weights()

    def prepare(value):
        arena = varena.Arena(0)
        region = arena.reserve(2 << 20)
        used = region.prepare()
        used.tensor.fill_(value)
        # Existing raw-fill preparation contract, outside the retirement interval.
        # This fixture does not claim asynchronous source adoption from this call.
        used.commit()
        stream.wait_stream(torch.cuda.current_stream())
        used.consume_on(stream.cuda_stream)
        with torch.cuda.stream(stream):
            assert bool(used.tensor.eq(value).all().item())
        stream.synchronize()
        return arena, region, used

    def observe_cache():
        # The allocation-style collector acquires real observer leases even if it
        # finishes from unrelated ready working memory before calling this owner.
        request = varena.request_cache_shrink(0, 1)
        assert request is not None

        def finished():
            facts = request.facts()
            return facts if facts["finished"] else None

        facts = until(finished)
        assert cache_owner.facts()["registered"]
        return facts

    try:
        until(
            lambda: (
                native.configure(shared=False, priority=original["priority"]),
                True,
            )[1]
        )
        baseline = publish()
        arena, region, used = prepare(79)
        arena_id, physical = arena.id, region.allocated_bytes
        until(lambda: True if publish() >= baseline + physical else None)
        cache_observed = observe_cache()
        assert publish() == baseline + physical
        reference = weakref.ref(arena)
        # Essential negative boundary: no close/request call precedes this deletion.
        del arena
        arena = None
        gc.collect()
        assert reference() is None
        assert (
            extra.facts()["active"]
            and cache_owner.facts()["registered"]
            and region.resident
        )
        with torch.cuda.stream(stream):
            assert bool(used.tensor.eq(79).all().item())
        escaped = used.tensor
        del used
        used = None
        gc.collect()
        assert region.resident and weights() >= baseline + physical
        del region
        region = None
        with torch.cuda.stream(stream):
            escaped.add_(0)
            final_event = torch.cuda.Event()
            final_event.record()
        del escaped
        escaped = None
        gc.collect()
        final_event.synchronize()
        until(lambda: True if publish() == baseline else None)
        assert extra.facts()["active"] and cache_owner.facts()["registered"]
        drop_only = {
            "arena_id": arena_id,
            "physical_bytes": physical,
            "no_prerequest": True,
            "escaped_region_and_pin_readback": True,
            "final_event_observed": True,
            "actual_weight_baseline": baseline,
            "cache_observation": cache_observed,
        }

        arena, region, used = prepare(83)
        plan = varena.Plan(arena, [("held", [region])])
        physical = region.allocated_bytes
        publish()
        driver = C.CDLL("libcuda.so.1")

        def fn(name, arguments):
            function = getattr(driver, name)
            function.argtypes = arguments
            function.restype = C.c_int
            return function

        create = fn("cuStreamCreate", [C.POINTER(C.c_void_p), C.c_uint])
        begin = fn("cuStreamBeginCapture_v2", [C.c_void_p, C.c_int])
        end = fn("cuStreamEndCapture", [C.c_void_p, C.POINTER(C.c_void_p)])
        destroy_graph = fn("cuGraphDestroy", [C.c_void_p])
        destroy_stream = fn("cuStreamDestroy_v2", [C.c_void_p])
        assert create(C.byref(raw_stream), 1) == 0
        with varena.capture(0):
            assert begin(raw_stream, 0) == 0
            captured = True
            before = extra.cuda_calls()
            ticket = arena.close()
            assert arena.closed and not ticket.try_complete()
            assert arena.close().facts()["arena_id"] == ticket.facts()["arena_id"]
            assert extra.cuda_calls() == before
            status = end(raw_stream, C.byref(graph))
            if status != 0:
                return Report(
                    False,
                    json.dumps(
                        {
                            "passed": False,
                            "capture_end_status": status,
                            "boundary": "executor must retire; no positive cleanup claim",
                        }
                    ),
                )
            captured = False
        try:
            arena.reserve(1)
        except RuntimeError as error:
            assert "closed" in str(error)
        else:
            raise AssertionError("closed root admitted another region")
        with torch.cuda.stream(stream):
            assert bool(used.tensor.eq(83).all().item())
        del arena, used, region
        arena = used = region = None
        gc.collect()
        stream.synchronize()
        assert not ticket.try_complete(), "genuine Plan reservation owner was revoked"
        assert ticket.facts()["remaining_physical_bytes"] == physical
        del plan
        plan = None
        gc.collect()
        until(lambda: True if ticket.try_complete() else None)
        assert ticket.facts()["remaining_physical_bytes"] == 0
        until(lambda: True if publish() == baseline else None)
        return Report(
            True,
            json.dumps(
                {
                    "passed": True,
                    "provenance": actual,
                    "torch": str(torch.__version__),
                    "torch_cuda": torch.version.cuda,
                    "drop_only": drop_only,
                    "checked_close": ticket.facts(),
                    "capture_native_driver_delta": 0,
                    "closed_reserve_refused": True,
                    "shared_client_survived": extra.facts(),
                    "cache_owner_survived": cache_owner.facts(),
                    "scope": "ordinary GPU request; final Python root disposal and actual recorded consumer completion; no source/whole-model qualification",
                },
                sort_keys=True,
            ),
        )
    finally:
        if captured:
            # Do not attempt ordinary GPU-owning cleanup after failed CUDA capture end.
            os._exit(3)
        if driver is not None:
            if graph.value:
                assert destroy_graph(graph) == 0
            if raw_stream.value:
                assert destroy_stream(raw_stream) == 0
        arena = used = escaped = region = plan = None  # noqa: F841 - Release all genuine owners before cleanup.
        gc.collect()
        cache_owner.close()
        extra.close()
        until(
            lambda: (
                native.configure(
                    shared=original["sharing"] == "Shared",
                    priority=original["priority"],
                ),
                True,
            )[1]
        )
