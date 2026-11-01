"""Real fork rejection after allocator installation, without any CUDA initialization."""
from __future__ import annotations

import ctypes
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import sys

import tensorfs
from tensorfs import _ext
import tensorfs.varena.torch as vt
import torch


class Failure(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_int32), ("cuda", ctypes.c_int32), ("message", ctypes.c_char * 512)]


class Bindings(ctypes.Structure):
    _fields_ = [("abi", ctypes.c_uint32), ("size", ctypes.c_uint32)] + [
        (name, ctypes.c_void_p) for name in (
            "alloc", "free", "record_stream", "trim", "pool_begin", "pool_end", "pool_release"
        )
    ]


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main(config):
    directory = Path(tensorfs.__file__).resolve().parent
    actual = {
        "tensorfs_version": version("tensorfs"), "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(directory / "_varena_torch.so"),
        "torch_version": torch.__version__, "torch_git": torch.version.git_version,
        "executable": sys.executable,
    }
    for key in ("tensorfs_version", "extension_sha256", "shim_sha256", "torch_version", "torch_git"):
        assert actual[key] == config[key], (key, config[key], actual[key])
    assert not torch.cuda.is_initialized()
    vt.install()  # no device argument: bind the process and callbacks, do not open CUDA
    table = Bindings.from_address(_ext._varena_working_bindings())
    assert table.abi == 1 and table.size == ctypes.sizeof(Bindings), (table.abi, table.size)
    pointer = ctypes.POINTER(Failure)
    allocate = ctypes.CFUNCTYPE(ctypes.c_uint64, ctypes.c_size_t, ctypes.c_int32, ctypes.c_uint64,
                               ctypes.c_int32, ctypes.c_uint64, ctypes.c_uint64, pointer)(table.alloc)
    free = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_int32, ctypes.c_uint64, ctypes.c_uint64, pointer)(table.free)
    record = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_uint64, ctypes.c_uint64, pointer)(table.record_stream)
    trim = ctypes.CFUNCTYPE(ctypes.c_int32, pointer)(table.trim)
    callbacks = {
        "zero-alloc": lambda error: allocate(0, 0, 0, 0, 0, 0, error),
        "zero-free": lambda error: free(0, 0, 0, error),
        "zero-record-stream": lambda error: record(0, 0, error),
        "trim-empty-registry": lambda error: trim(error),
    }
    assert not torch.cuda.is_initialized()
    results = {}
    # A fresh fork for each operation means a destructor's error latch cannot mask the next check.
    for name in ("explicit-check", "stats-empty-registry", *callbacks):
        read_fd, write_fd = os.pipe()
        pid = os.fork()
        if pid == 0:
            os.close(read_fd)
            try:
                reason = ""
                if name in callbacks:
                    failure = Failure()
                    callbacks[name](ctypes.byref(failure))
                    assert failure.kind != 0, f"{name} accepted an inherited allocator"
                    reason = bytes(failure.message).decode()
                else:
                    try:
                        if name == "explicit-check":
                            _ext._varena_working_check()
                        else:
                            _ext._varena_working_stats(0)
                    except BaseException as error:
                        reason = str(error)
                assert "after fork" in reason, (name, reason)
                assert not torch.cuda.is_initialized()
                result = {"passed": True, "reason": reason, "cuda_initialized": False}
            except BaseException as error:
                result = {"passed": False, "error": f"{type(error).__name__}: {error}"}
            with os.fdopen(write_fd, "w") as target:
                json.dump(result, target)
            os._exit(0 if result["passed"] else 1)
        os.close(write_fd)
        with os.fdopen(read_fd) as source:
            result = json.load(source)
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0 and result["passed"], (name, status, result)
        results[name] = result
    _ext._varena_working_check()  # rejected children did not poison the parent
    unchanged = _ext._varena_working_stats(0)
    assert all(value == 0 for value in unchanged.values()), unchanged
    assert not torch.cuda.is_initialized()
    return {"passed": True, "provenance": actual, "cases": results,
            "cuda_initialized": False, "parent_stats": unchanged}


if __name__ == "__main__":
    try:
        print(json.dumps({"result": main(json.loads(sys.argv[1]))}, sort_keys=True), flush=True)
    except BaseException as error:
        print(json.dumps({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}}), flush=True)
        raise
