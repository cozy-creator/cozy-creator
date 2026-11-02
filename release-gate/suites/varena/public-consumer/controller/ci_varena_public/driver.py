"""Actual caller-owned CUDA streams, pinned readback and same-core fixture buffers."""

from __future__ import annotations

import ctypes as C
import json
import time
from pathlib import Path


class ObservationBusy(Exception):
    """A typed nonblocking shared-metadata observation deferred."""


def until(predicate, *, seconds=20, label="native completion"):
    end = time.monotonic() + seconds
    last = None
    while True:
        try:
            last = predicate()
        except ObservationBusy:
            last = None
        if last:
            return last
        if time.monotonic() >= end:
            raise AssertionError(
                f"{label} did not reach its observed predicate: {last!r}"
            )
        time.sleep(0.005)


class Probe:
    def __init__(self, path: Path):
        self.lib = C.CDLL(str(path))
        for name, args in {
            "probe_inventory": [C.c_int, C.POINTER(C.c_uint64)],
            "probe_alloc": [C.c_int, C.c_size_t, C.c_uint64, C.POINTER(C.c_uint64)],
            "probe_free": [C.c_int, C.c_uint64, C.c_uint64],
            "probe_retire_stream": [C.c_int, C.c_uint64],
            "probe_trim": [C.c_int],
            "probe_observer": [C.c_int, C.c_char_p],
        }.items():
            f = getattr(self.lib, name)
            f.argtypes, f.restype = args, C.c_int
        self.lib.probe_core_identity.restype = C.c_uint64
        self.lib.probe_last_error.restype = C.c_char_p
        self.lib.probe_observer_facts.restype = C.c_char_p
        self.lib.probe_working_stats.argtypes = [C.c_int]
        self.lib.probe_working_stats.restype = C.c_char_p
        self.lib.probe_table_snapshot.argtypes = [C.c_char_p, C.c_char_p]
        self.lib.probe_table_snapshot.restype = C.c_char_p

    def call(self, name, *args):
        result = getattr(self.lib, "probe_" + name)(*args)
        if result == 3:
            raise ObservationBusy(name)
        if result:
            raise RuntimeError(f"{name}: {self.lib.probe_last_error().decode()}")

    def inventory(self, device=0):
        value = C.c_uint64()
        self.call("inventory", device, C.byref(value))
        return value.value

    def allocate(self, bytes, stream, device=0):
        value = C.c_uint64()
        self.call("alloc", device, bytes, stream.handle, C.byref(value))
        return value.value

    def working_stats(self, device=0):
        value = json.loads(self.lib.probe_working_stats(device))
        if "error" in value:
            raise RuntimeError(value["error"])
        return value

    def facts(self):
        value = json.loads(self.lib.probe_observer_facts())
        if value.get("deferred"):
            raise ObservationBusy("ledger snapshot")
        if "error" in value:
            raise RuntimeError(value["error"])
        return value


class Driver:
    def __init__(self, *, retain_primary=False, device=0):
        self.lib = C.CDLL("libcuda.so.1")
        obj = C.c_void_p
        for name, args in {
            "cuInit": [C.c_uint],
            "cuDevicePrimaryCtxRetain": [C.POINTER(obj), C.c_int],
            "cuCtxPushCurrent_v2": [obj],
            "cuCtxPopCurrent_v2": [C.POINTER(obj)],
            "cuDevicePrimaryCtxRelease_v2": [C.c_int],
            "cuDeviceGetUuid_v2": [obj, C.c_int],
            "cuStreamCreate": [C.POINTER(obj), C.c_uint],
            "cuStreamDestroy_v2": [obj],
            "cuStreamQuery": [obj],
            "cuEventCreate": [C.POINTER(obj), C.c_uint],
            "cuEventRecord": [obj, obj],
            "cuEventSynchronize": [obj],
            "cuEventDestroy_v2": [obj],
            "cuMemAllocHost_v2": [C.POINTER(obj), C.c_size_t],
            "cuMemFreeHost": [obj],
            "cuMemcpyDtoHAsync_v2": [obj, C.c_uint64, C.c_size_t, obj],
            "cuMemcpyDtoDAsync_v2": [C.c_uint64, C.c_uint64, C.c_size_t, obj],
            "cuMemsetD8Async": [C.c_uint64, C.c_ubyte, C.c_size_t, obj],
            "cuStreamWaitValue32_v2": [obj, C.c_uint64, C.c_uint32, C.c_uint],
            "cuStreamWriteValue32_v2": [obj, C.c_uint64, C.c_uint32, C.c_uint],
        }.items():
            f = getattr(self.lib, name)
            f.argtypes, f.restype = args, C.c_int
        self.context = None
        self.device = device
        if retain_primary:
            context = obj()
            self.call("cuInit", 0)
            self.call("cuDevicePrimaryCtxRetain", C.byref(context), device)
            self.call("cuCtxPushCurrent_v2", context)
            self.context = context

    def call(self, name, *args):
        code = getattr(self.lib, name)(*args)
        if code:
            raise RuntimeError(f"{name}: CUDA {code}")

    def uuid(self):
        value = (C.c_ubyte * 16)()
        self.call("cuDeviceGetUuid_v2", value, self.device)
        return bytes(value).hex()

    def close(self):
        if self.context is not None:
            value = C.c_void_p()
            self.call("cuCtxPopCurrent_v2", C.byref(value))
            assert value.value == self.context.value
            self.call("cuDevicePrimaryCtxRelease_v2", self.device)
            self.context = None


class Stream:
    def __init__(self, driver: Driver, probe: Probe):
        self.driver, self.probe = driver, probe
        self.pointer, self.done = C.c_void_p(), C.c_void_p()
        driver.call("cuStreamCreate", C.byref(self.pointer), 1)
        driver.call("cuEventCreate", C.byref(self.done), 2)
        assert self.handle > 2

    @property
    def handle(self):
        return self.pointer.value

    def finish(self):
        self.driver.call("cuEventRecord", self.done, self.pointer)
        self.driver.call("cuEventSynchronize", self.done)

    def pending(self):
        status = self.driver.lib.cuStreamQuery(self.pointer)
        assert status in (0, 600), status
        return status == 600

    def fill(self, ptr, count, value):
        self.driver.call("cuMemsetD8Async", ptr, value, count, self.pointer)

    def copy(self, dst, src, count):
        self.driver.call("cuMemcpyDtoDAsync_v2", dst, src, count, self.pointer)

    def read(self, ptr, count):
        host = C.c_void_p()
        self.driver.call("cuMemAllocHost_v2", C.byref(host), count)
        # On uncertain copy/completion, preserve host storage for the failed process;
        # do not release a possible in-flight borrower in a masking finally block.
        self.driver.call("cuMemcpyDtoHAsync_v2", host, ptr, count, self.pointer)
        self.finish()
        result = C.string_at(host, count)
        self.driver.call("cuMemFreeHost", host)
        return result

    def write_gate(self, gate, value):
        self.driver.call("cuStreamWriteValue32_v2", self.pointer, gate, value, 0)

    def wait_gate(self, gate):
        self.driver.call("cuStreamWaitValue32_v2", self.pointer, gate, 1, 1)  # EQ

    def close(self):
        self.finish()
        self.probe.call("retire_stream", self.driver.device, self.handle)
        self.driver.call("cuEventDestroy_v2", self.done)
        self.driver.call("cuStreamDestroy_v2", self.pointer)
