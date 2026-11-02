"""Raw final-close first, then Runtime custody on a new generation; no Torch allocator."""

import ctypes as C
import hashlib
import json
import os
import sys
import time
import traceback
from importlib.metadata import version
from pathlib import Path


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main():
    import tensorfs
    from cozy_runtime.internal import cooperation
    from tensorfs import _ext
    from tensorfs.varena import _runtime as varena

    config = json.loads(sys.argv[1])
    actual = {
        "runtime_version": version("cozy-runtime"),
        "runtime_owner_sha256": digest(cooperation.__file__),
        "tensorfs_version": version("tensorfs"),
        "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(Path(tensorfs.__file__).parent / "_varena_torch.so"),
    }
    assert all(actual[key] == config[key] for key in actual), actual
    varena.CooperationOwner.initialize()
    a = b = counter = verification = runtime_seed = runtime = None
    driver = None
    context = C.c_void_p()
    stream = C.c_void_p()
    graph = C.c_void_p()
    pushed = captured = False

    def until(operation):
        end = time.monotonic() + 10
        while True:
            result = operation()
            if result:
                return result
            assert time.monotonic() < end, "shared-owner protocol did not settle"
            time.sleep(0.005)

    def row(owner):
        table = owner.snapshot()
        value = next(value for value in table["rows"] if value["id"] == table["owner"])
        assert value["pid"] == os.getpid() and owner.facts()["owner"] == value["id"]
        return value

    def fn(name, args):
        function = getattr(driver, name)
        function.argtypes = args
        function.restype = C.c_int
        return function

    def check(status):
        assert status == 0, f"CUDA status {status}"

    try:
        a = varena.CooperationOwner(
            0, config["gpu_uuid"], config["directory"], shared=True, priority=7
        )
        first = row(a)
        assert first["sharing"] == "Shared" and first["priority"] == 7
        b = varena.CooperationOwner(
            0, config["gpu_uuid"], config["directory"], shared=False, priority=-99
        )
        inherited = row(b)
        assert (
            inherited["id"] == first["id"]
            and inherited["sharing"] == "Shared"
            and inherited["priority"] == 7
        )
        assert a.facts()["clients"] == b.facts()["clients"] == 2
        assert a.facts()["passive_holds"] == b.facts()["passive_holds"] == 0

        before = b.facts()["serviced"]
        a.close()
        closed_a = a.facts()
        assert closed_a["closed"] and not closed_a["active"] and closed_a["clients"] == 1
        b.wake(used=True)
        continued = until(lambda: b.facts() if b.facts()["serviced"] > before else None)
        assert continued["active"] and not continued["closed"] and row(b)["id"] == first["id"]

        # Manual Peer remains a separate explicit API, used here only as a metadata
        # counter witness. It is not another automatic service client.
        counter_dir = Path(config["directory"]) / "counter"
        counter_dir.mkdir(mode=0o700)
        counter = varena.Cooperation(0, config["gpu_uuid"], counter_dir, shared=False, priority=0)
        assert b.facts()["clients"] == 1
        driver = C.CDLL("libcuda.so.1")
        retain = fn("cuDevicePrimaryCtxRetain", [C.POINTER(C.c_void_p), C.c_int])
        push = fn("cuCtxPushCurrent_v2", [C.c_void_p])
        pop = fn("cuCtxPopCurrent_v2", [C.POINTER(C.c_void_p)])
        release = fn("cuDevicePrimaryCtxRelease_v2", [C.c_int])
        create = fn("cuStreamCreate", [C.POINTER(C.c_void_p), C.c_uint])
        destroy_stream = fn("cuStreamDestroy_v2", [C.c_void_p])
        begin = fn("cuStreamBeginCapture_v2", [C.c_void_p, C.c_int])
        end = fn("cuStreamEndCapture", [C.c_void_p, C.POINTER(C.c_void_p)])
        destroy_graph = fn("cuGraphDestroy", [C.c_void_p])
        check(retain(C.byref(context), 0))
        check(push(context))
        pushed = True
        check(create(C.byref(stream), 1))  # CU_STREAM_NON_BLOCKING
        with varena.capture(0):
            check(begin(stream, 0))
            captured = True
            before_driver = counter.cuda_calls()
            try:
                b.close()
            except varena.CooperationDeferredError:
                pass
            else:
                raise AssertionError(
                    "final close did not retain its capture-excluded retirement ticket"
                )
            pending = b.facts()
            assert (
                pending["closed"]
                and pending["stopping"]
                and not pending["retired"]
                and pending["clients"] == 0
            )
            assert counter.cuda_calls() == before_driver
            status = end(stream, C.byref(graph))
            if status != 0:
                emit(
                    {
                        "passed": False,
                        "capture_end_status": status,
                        "boundary": "exit without GPU-owning teardown",
                    }
                )
                os._exit(3)
            captured = False
        b.close()  # Same native object owns the retry; no reconstruction/reset.
        retired = b.facts()
        assert retired["closed"] and retired["retired"] and retired["clients"] == 0
        verification = varena.Cooperation(
            0, config["gpu_uuid"], config["directory"], shared=False, priority=0
        )
        retired_table = verification.snapshot()
        assert all(item["id"] != first["id"] for item in retired_table["rows"])
        assert retired_table["owner"] != first["id"]

        # Keep the raw service's final-close proof above uncontaminated by Runtime's
        # intentional process-lifetime keeper. Use a new service generation for
        # actual Runtime constructor adoption/mismatch; do not weaken row-gone above.
        verification.close()  # The row-gone witness must release its own participant.
        runtime_seed = varena.CooperationOwner(
            0, config["gpu_uuid"], config["directory"], shared=True, priority=7
        )
        runtime_row = row(runtime_seed)
        assert runtime_row["id"] != first["id"]
        failures = []
        same = cooperation.Config(config["gpu_uuid"], Path(config["directory"]), True, 7)
        runtime = cooperation.Owner(0, same, failures.append)
        assert runtime.facts()["owner"] == runtime_row["id"] and runtime.facts()["clients"] == 2
        assert runtime.facts()["passive_holds"] == 1
        runtime.check()
        runtime.close()
        assert runtime_seed.facts()["clients"] == 1 and runtime_seed.facts()["active"]
        measured = runtime_seed.facts()["pressure"]["measured_requests"]
        mismatch = cooperation.Config(config["gpu_uuid"], Path(config["directory"]), False, 81)
        try:
            cooperation.Owner(0, mismatch, failures.append)
        except cooperation.Failure as error:
            assert error.varena_no_retry and "differs from negotiated Start" in str(error)
            rejected = str(error)
        else:
            raise AssertionError("Runtime silently adopted or overwrote mismatched lifecycle settings")
        assert row(runtime_seed)["sharing"] == "Shared" and row(runtime_seed)["priority"] == 7
        assert runtime_seed.facts()["clients"] == 1
        assert runtime_seed.facts()["pressure"]["measured_requests"] == measured
        assert not failures
        runtime_seed.close()
        custody = runtime_seed.facts()
        assert custody["closed"] and custody["clients"] == 0 and custody["passive_holds"] == 1
        assert not custody["stopping"] and not custody["retired"] and custody["activity"] == "Unknown"
        custody_table = verification.snapshot()
        assert any(value["id"] == runtime_row["id"] for value in custody_table["rows"])
        result = {
            "passed": True,
            "case": "raw_owner_final_close_then_runtime_custody",
            "provenance": actual,
            "pid": os.getpid(),
            "row": first["id"],
            "initial": first,
            "inherited": inherited,
            "runtime_same_config_adopted": True,
            "runtime_row": runtime_row,
            "runtime_custody": custody,
            "runtime_custody_table": custody_table,
            "runtime_mismatch_refused": rejected,
            "failed_start_did_not_train_H": True,
            "closed_first": closed_a,
            "continued": continued,
            "capture_pending": pending,
            "retired": retired,
            "retired_original_table": retired_table,
            "native_driver_calls_during_final_close": 0,
            "scope": "raw native/CUDA-only final-close on the first service generation; Runtime constructor/custody on a later generation; no Torch allocator and no claim that Runtime custody retires before process exit",
        }
    finally:
        if captured:
            emit(
                {
                    "passed": False,
                    "boundary": "unclosed CUDA capture; exit without native owner destruction",
                }
            )
            os._exit(3)
        if driver is not None:
            if graph.value:
                check(destroy_graph(graph))
            if stream.value:
                check(destroy_stream(stream))
        for owner in (runtime, runtime_seed, a, b, counter, verification):
            if owner is not None:
                owner.close()
        if pushed:
            popped = C.c_void_p()
            check(pop(C.byref(popped)))
            check(release(0))
    emit(result)


try:
    main()
except BaseException as error:
    emit(
        {
            "passed": False,
            "error": f"{type(error).__name__}: {error}",
            "traceback": traceback.format_exc(),
        }
    )
    raise
