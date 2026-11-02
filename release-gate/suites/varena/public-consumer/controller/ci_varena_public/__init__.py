"""Ordinary Cozy control of the exact public C/Rust/Python consumer cohort."""

from __future__ import annotations

import ctypes as C
import hashlib
import importlib.metadata
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

from .driver import Driver, Stream, until
from .driver import Probe as NativeProbe

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    source_sha: str
    gpu_uuid: str
    tensorfs_version: str
    runtime_version: str
    extension_sha256: str
    shim_sha256: str
    core_sha256: str
    std_name: str
    std_sha256: str
    helper_sha256: str
    mixed_c_sha256: str
    mixed_rust_sha256: str
    tensorfs_files: dict[str, str]
    runtime_files: dict[str, str]
    source_helper_sha256: str = ""
    timeout_s: float = 90.0


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def uuid_key(value):
    return value.lower().removeprefix("gpu-").removeprefix("mig-").replace("-", "")


def setup(payload, ctx):
    import cozy_runtime
    import tensorfs
    import torch
    from cozy_runtime.internal import int8_linear
    from cozy_runtime.internal.residency import _REQUEST
    from tensorfs import _ext, varena

    assert 0 < payload.timeout_s <= 120
    assert payload.gpu_uuid != "UNBOUND" and uuid_key(
        str(torch.cuda.get_device_properties(0).uuid)
    ) == uuid_key(payload.gpu_uuid)
    assert ctx.device.type == "cuda" and (ctx.device.index or 0) == 0
    request = _REQUEST.get()
    assert request is not None and request.id == ctx.request_id and not request.idle
    assert request.memory.cooperation is not None
    roots = {
        "tensorfs": Path(tensorfs.__file__).parent,
        "runtime": Path(cozy_runtime.__file__).parent,
    }
    actual = {
        "tensorfs_version": importlib.metadata.version("tensorfs"),
        "runtime_version": importlib.metadata.version("cozy-runtime"),
        "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(roots["tensorfs"] / "_varena_torch.so"),
    }
    assert all(actual[name] == getattr(payload, name) for name in actual), actual
    for root, hashes in (
        (roots["tensorfs"], payload.tensorfs_files),
        (roots["runtime"], payload.runtime_files),
    ):
        for name, expected in hashes.items():
            assert digest(root / name) == expected, name
    binaries = Path(__file__).parent / "bin"
    manifest = json.loads((binaries / "manifest.json").read_text())
    assert manifest["source_sha"] == payload.source_sha
    for name, expected in (
        ("public-probe.so", payload.helper_sha256),
        ("mixed-c.so", payload.mixed_c_sha256),
        ("mixed-rust.so", payload.mixed_rust_sha256),
    ):
        assert digest(binaries / name) == expected == manifest["binaries"][name]
    helper = NativeProbe(binaries / "public-probe.so")
    c, rust = (
        C.CDLL(str(binaries / "mixed-c.so")),
        C.CDLL(str(binaries / "mixed-rust.so")),
    )
    for lib, name in (
        (c, "mixed_c_identity"),
        (rust, "mixed_rust_identity"),
        (rust, "mixed_internal_identity"),
    ):
        getattr(lib, name).restype = C.c_uint64
    identities = [
        _ext._varena_core_identity(),
        varena.core_identity(),
        helper.lib.probe_core_identity(),
        c.mixed_c_identity(),
        rust.mixed_rust_identity(),
        rust.mixed_internal_identity(),
    ]
    assert identities[0] and len(set(identities)) == 1, identities
    mapped = {
        line.split(maxsplit=5)[-1]
        for line in Path("/proc/self/maps").read_text().splitlines()
        if len(line.split(maxsplit=5)) == 6
    }
    libraries = {
        "libvarena.so": payload.core_sha256,
        payload.std_name: payload.std_sha256,
    }
    rust_std = {
        Path(value).name
        for value in mapped
        if Path(value).name.startswith("libstd-") and value.endswith(".so")
    }
    assert rust_std == {payload.std_name}, rust_std
    for name, expected in manifest["python_helpers"].items():
        assert digest(binaries / name) == expected, name
    for name, expected in libraries.items():
        paths = {value for value in mapped if Path(value).name == name}
        assert len(paths) == 1 and digest(next(iter(paths))) == expected, (name, paths)
        assert digest(binaries / "lib" / name) == expected
    # Wait for the executor's actual independent kernel-readiness producer before baselines.
    int8_linear.native(torch, 0)
    ready = torch.cuda.Event()
    ready.record(torch.cuda.current_stream(0))
    ready.synchronize()
    return (
        request,
        helper,
        c,
        rust,
        {
            "actual": actual,
            "identities": identities,
            "pid": os.getpid(),
            "request_id": ctx.request_id,
            "manifest": manifest,
            "input": msgspec.to_builtins(payload),
            "libraries": libraries,
        },
    )


def drain(api, *, baseline_quarantined=0):
    def complete():
        value = api.retirement_stats()
        assert value["quarantined"] == baseline_quarantined, value
        return value if value["queued"] == 0 else None

    return until(complete, label="actual API retirement queue")


def refused(call):
    from tensorfs import varena

    try:
        call()
    except varena.Invalid:
        return
    raise AssertionError("public default-stream sentinel was accepted")


def run_basic(payload, ctx, result):
    from tensorfs import varena
    from tensorfs.varena import api

    request, helper, c, rust, provenance = setup(payload, ctx)
    result["provenance"] = provenance
    native_owner = request.memory.cooperation._peer
    drain(varena)
    driver = Driver()
    stream = Stream(driver, helper)
    before_owner = native_owner.facts()
    before_table = native_owner.snapshot()
    epoch0 = helper.inventory()
    first = varena.open(0)
    epoch1 = helper.inventory()
    second = varena.open(0)
    epoch2 = helper.inventory()
    opened = native_owner.facts()
    table = native_owner.snapshot()
    result["opens"] = {
        "inventory": [epoch0, epoch1, epoch2],
        "before": before_owner,
        "after": opened,
        "before_owner": before_table["owner"],
        "owner": table["owner"],
    }
    assert epoch1 == epoch0 + 1 and epoch2 == epoch1 + 1, result["opens"]
    assert opened["clients"] == before_owner["clients"] + 2
    assert opened["passive_holds"] == before_owner["passive_holds"] == 1
    assert table["owner"] == before_table["owner"]
    baseline = first.stats()

    cold = first.region(bytes(range(256)) * 3)
    cache = first.cache(8 << 20)
    page = cache.info()["granularity"]
    cache.commit(0, page)
    stream.fill(cache.info()["base"], page, 119)
    live = first.alloc(1024, stream.handle)
    stream.fill(live.ptr, 1024, 37)
    stream.finish()
    stable = (
        "weights_resident",
        "working_live",
        "working_capacity",
        "working_mapped",
        "caches",
        "cache_live",
        "cache_pending",
        "cache_ready",
        "allocations",
        "frees",
        "driver_calls",
    )
    result["sentinels"] = []
    for sentinel in (0, 1, 2):
        before = first.stats()
        cache_before = cache.info()
        out = C.c_uint64(2**64 - 1)
        code = api._lib.vr_v1_alloc(first._handle, 1024, sentinel, C.byref(out))
        assert code == 5 and out.value == 0, (sentinel, code, out.value)
        refused(lambda sentinel=sentinel: first.alloc(1024, sentinel))
        refused(lambda sentinel=sentinel: first.use([cold], sentinel))
        refused(lambda sentinel=sentinel: live.record_stream(sentinel))
        refused(lambda sentinel=sentinel: cache.release(0, page, sentinel))
        after = first.stats()
        record = {
            "stream": sentinel,
            "before": before,
            "after": after,
            "cache_before": cache_before,
            "cache_after": cache.info(),
            "constructor_status": code,
            "constructor_output": out.value,
        }
        result["sentinels"].append(record)
        assert all(before[key] == after[key] for key in stable), record
        assert record["cache_before"] == record["cache_after"], record
    assert stream.read(live.ptr, 1024) == bytes([37]) * 1024
    assert stream.read(cache.info()["base"], page) == bytes([119]) * page
    live.close()
    cache.release(0, page, stream.handle)
    stream.finish()
    assert cache.info()["mapped"] == 0
    cache.close()
    cold.close()

    a = bytes(range(256)) * 8193
    b = bytes(reversed(range(256))) * 4097
    left, right = first.region(a), second.region(b)
    plan = first.plan({"both": [left, right]})
    key = f"public/{ctx.request_id}/bytes/{len(a)}/{len(b)}"
    result["readbacks"] = []
    with plan.phase("both", key + "/concat"):
        used = second.use([left, right], stream.handle)
        pointers = [used.ptr(0), used.ptr(1)]
        scratch = first.alloc(len(a) + len(b), stream.handle)
        stream.copy(scratch.ptr, pointers[0], len(a))
        stream.copy(scratch.ptr + len(a), pointers[1], len(b))
        read = stream.read(scratch.ptr, len(a) + len(b))
        assert read == a + b
        result["readbacks"].append(
            {
                "pass": "cold",
                "bytes": len(read),
                "sha256": hashlib.sha256(read).hexdigest(),
                "pointers": pointers,
            }
        )
        pinned = first.stats()
        first.trim()
        assert first.stats()["weights_resident"] == pinned["weights_resident"]
        scratch.close()
        used.close()
        stream.finish()
    drain(varena)  # actual async Source/Fill retirement, not only consumer completion
    resident = first.stats()["weights_resident"]
    first.trim()
    evicted = first.stats()
    result["eviction"] = {"resident_before": resident, "after": evicted}
    assert (
        resident > baseline["weights_resident"]
        and evicted["weights_resident"] == baseline["weights_resident"]
    )
    with plan.phase("both", key + "/read"):
        kept = second.use([left, right], stream.handle)
        assert [kept.ptr(0), kept.ptr(1)] == pointers
        assert stream.read(kept.ptr(0), len(a)) == a
        assert stream.read(kept.ptr(1), len(b)) == b
        result["readbacks"].append(
            {"pass": "refill", "bytes": len(a) + len(b), "pointers": pointers}
        )
    left.close()
    right.close()
    first.close()
    second.close()
    result["retained"] = {
        "closed_python_handles": [
            left._handle,
            right._handle,
            first._handle,
            second._handle,
        ],
        "use_and_plan": native_owner.facts(),
    }
    assert result["retained"]["closed_python_handles"] == [0, 0, 0, 0]
    assert (
        stream.read(kept.ptr(0), len(a)) == a and stream.read(kept.ptr(1), len(b)) == b
    )
    kept.close()
    stream.finish()
    result["retained"]["plan_only"] = native_owner.facts()
    assert result["retained"]["plan_only"]["clients"] == before_owner["clients"] + 2
    plan.close()
    result["retirement"] = drain(varena)
    final_owner = native_owner.facts()
    assert (
        final_owner["clients"] == before_owner["clients"]
        and final_owner["passive_holds"] == 1
    )

    # The existing actual mixed clients use the same borrowed stream and shared core.
    with varena.open(0) as mixed:
        c.mixed_c_alloc.argtypes = [
            C.c_uint64,
            C.c_uint64,
            C.c_uint64,
            C.POINTER(C.c_uint64),
        ]
        c.mixed_c_free.argtypes = [C.c_uint64]
        rust.mixed_rust_gpu.argtypes = [C.c_uint64]
        out = C.c_uint64()
        previous = mixed.stats()
        assert c.mixed_c_alloc(mixed._handle, 1 << 20, stream.handle, C.byref(out)) == 0
        pointer = api._new("allocation_ptr", out.value)
        stream.fill(pointer, 1 << 20, 53)
        assert stream.read(pointer, 1 << 20) == bytes([53]) * (1 << 20)
        assert mixed.stats()["working_live"] == previous["working_live"] + (1 << 20)
        assert rust.mixed_rust_gpu(stream.handle) == 1
        assert c.mixed_c_free(out.value) == 0
        stream.finish()
        mixed.trim()
        result["mixed"] = {
            "before": previous,
            "after": mixed.stats(),
            "full_readback_bytes": 1 << 20,
        }
    drain(varena)
    stream.finish()
    helper.call("trim", 0)
    with varena.open(0) as final_inspector:
        result["final"] = final_inspector.stats()
        for name in (
            "weights_resident",
            "working_live",
            "working_capacity",
            "caches",
            "cache_live",
            "cache_pending",
            "cache_ready",
        ):
            assert result["final"][name] == baseline[name], (
                name,
                baseline,
                result["final"],
            )
        assert result["final"]["working_mapped"] <= baseline["working_mapped"]
    drain(varena)
    stream.close()
    driver.close()
    result["owner_final"] = native_owner.facts()
    result["table_final"] = native_owner.snapshot()
    assert result["table_final"]["owner"] == before_table["owner"]


@app.entrypoint(accelerator=True)
def public_basic(payload: Probe, ctx: Context) -> Report:
    result = {"passed": False, "case": "public_basic"}
    try:
        run_basic(payload, ctx, result)
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve the complete failed probe receipt
        result["error"] = repr(error)
        result["traceback"] = traceback.format_exc()
        # No failure cleanup guesses that partially submitted work is complete.
        # The ordinary executor marks this false result; root owns failed-worker cleanup.
    return Report(result["passed"], json.dumps(result, sort_keys=True))


def child_command(command, payload, ctx, result, *, report_field="passed"):
    """Own one real child by pidfd, preserving complete stdout/error before verdict."""
    child = subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    pidfd = os.pidfd_open(child.pid)
    started = time.monotonic()
    stdout = stderr = ""
    try:
        while True:
            ctx.raise_if_cancelled()
            try:
                stdout, stderr = child.communicate(timeout=0.25)
                break
            except subprocess.TimeoutExpired:
                if time.monotonic() - started > payload.timeout_s:
                    raise TimeoutError(
                        "public fixture exceeded its declared child deadline"
                    )
    finally:
        if child.poll() is None:
            signal.pidfd_send_signal(pidfd, signal.SIGTERM)
            try:
                stdout, stderr = child.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                stdout, stderr = child.communicate(timeout=5)
        result["child"] = {
            "command": command,
            "pid": child.pid,
            "returncode": child.returncode,
            "stdout": stdout,
            "stderr": stderr,
            "elapsed_s": time.monotonic() - started,
            "exited": child.poll() is not None,
        }
        os.close(pidfd)
    assert child.returncode == 0, result["child"]
    rows = [json.loads(line) for line in stdout.splitlines() if line.startswith("{")]
    assert rows and rows[-1].get(report_field) is True, result["child"]
    result["native_result"] = rows[-1]


@app.entrypoint(accelerator=True)
def passive_custody(payload: Probe, ctx: Context) -> Report:
    result = {"passed": False, "case": "passive_custody"}
    try:
        *_, result["provenance"] = setup(payload, ctx)
        with tempfile.TemporaryDirectory(prefix="public-custody-") as folder:
            source = Path(folder) / "input.json"
            source.write_text(json.dumps(msgspec.to_builtins(payload)))
            child_command(
                [
                    sys.executable,
                    "-I",
                    "-B",
                    str(Path(__file__).with_name("child.py")),
                    str(source),
                ],
                payload,
                ctx,
                result,
            )
        assert result["native_result"]["pid"] == result["child"]["pid"]
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve the complete failed probe receipt
        result["error"], result["traceback"] = repr(error), traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))


def source_case(payload, ctx, mode):
    result = {"passed": False, "case": "public_source", "mode": mode}
    try:
        *_, result["provenance"] = setup(payload, ctx)
        binary = Path(__file__).parent / "bin/source-c"
        assert (
            payload.source_helper_sha256
            and digest(binary) == payload.source_helper_sha256
        )
        child_command([str(binary), mode, payload.gpu_uuid, "0"], payload, ctx, result)
        assert result["native_result"]["mode"] == mode
        assert uuid_key(result["native_result"]["gpu_uuid"]) == uuid_key(
            payload.gpu_uuid
        )
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve the complete failed probe receipt
        result["error"], result["traceback"] = repr(error), traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def source_ordering(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "ordering")


@app.entrypoint(accelerator=True)
def source_partial(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "partial")


@app.entrypoint(accelerator=True)
def source_prepare_error(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "prepare_error")


@app.entrypoint(accelerator=True)
def source_pending(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "source_pending")


@app.entrypoint(accelerator=True)
def source_retained(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "retained")


@app.entrypoint(accelerator=True)
def host_bytes(payload: Probe, ctx: Context) -> Report:
    return source_case(payload, ctx, "host_bytes")


@app.entrypoint(accelerator=True)
def mixed_retirement(payload: Probe, ctx: Context) -> Report:
    result = {"passed": False, "case": "mixed_retirement"}
    try:
        *_, result["provenance"] = setup(payload, ctx)
        child_command(
            [
                sys.executable,
                "-I",
                "-B",
                str(Path(__file__).with_name("retirement_runner.py")),
            ],
            payload,
            ctx,
            result,
            report_field="gpu",
        )
        assert result["native_result"]["callback_release_during_retirement"]
        assert result["native_result"]["live_cache_quarantine"]["quarantined"] > 0
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve the complete failed probe receipt
        result["error"], result["traceback"] = repr(error), traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))


def mixed_order(payload, ctx, order):
    result = {"passed": False, "case": "mixed_load_order", "order": order}
    try:
        *_, result["provenance"] = setup(payload, ctx)
        with tempfile.TemporaryDirectory(prefix="public-mixed-order-") as folder:
            source = Path(folder) / "input.json"
            source.write_text(json.dumps(msgspec.to_builtins(payload)))
            child_command(
                [
                    sys.executable,
                    "-I",
                    "-B",
                    str(Path(__file__).with_name("mixed_runner.py")),
                    str(source),
                    order,
                ],
                payload,
                ctx,
                result,
            )
        assert result["native_result"]["order"] == order
        assert uuid_key(result["native_result"]["gpu_uuid"]) == uuid_key(
            payload.gpu_uuid
        )
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve the complete failed probe receipt
        result["error"], result["traceback"] = repr(error), traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def mixed_native_first(payload: Probe, ctx: Context) -> Report:
    return mixed_order(payload, ctx, "native-first")


@app.entrypoint(accelerator=True)
def mixed_python_first(payload: Probe, ctx: Context) -> Report:
    return mixed_order(payload, ctx, "python-first")
