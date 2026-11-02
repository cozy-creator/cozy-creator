"""Fresh-child allocator timing through Torch's common raw allocation entrypoints.

No kernels or device copies occur in the timed loop. Python dispatch is included for both
backends; CUDA initialization, warmup, telemetry and synchronization are outside that loop.
"""
from __future__ import annotations

import gc
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import platform
import statistics
import sys
import time

import tensorfs
from tensorfs import _ext
import tensorfs.varena.torch as vt
import torch

MIB = 1 << 20
MAX_MAPPED = 256 * MIB
SIZES = (512, 513, 1536, 4096, 65536, MIB, MIB + 17, 8 * MIB)


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main(config):
    mode = config["mode"]
    assert mode in ("native", "varena"), mode
    assert config["samples"] >= 5
    assert config["rounds"] >= 10_000
    installed = Path(tensorfs.__file__).resolve().parent
    actual = {
        "tensorfs_version": version("tensorfs"),
        "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(installed / "_varena_torch.so"),
        "extension_path": str(Path(_ext.__file__).resolve()),
        "torch": torch.__version__, "torch_git": torch.version.git_version,
        "torch_distribution": version("torch"), "torch_cuda": torch.version.cuda,
        "torch_cxx11_abi": bool(torch._C._GLIBCXX_USE_CXX11_ABI),
        "python": sys.version, "executable": sys.executable, "platform": platform.platform(),
        "visible_devices": os.environ.get("CUDA_VISIBLE_DEVICES"),
        "allocator_conf": os.environ.get("PYTORCH_ALLOC_CONF"),
        "cuda_allocator_conf": os.environ.get("PYTORCH_CUDA_ALLOC_CONF"),
    }
    assert actual["torch"] == config["torch_version"], (config["torch_version"], actual["torch"])
    assert actual["torch_cuda"] == "13.0", actual
    assert actual["torch_distribution"] == "2.14.0", actual
    assert actual["torch_git"] == config["torch_git"], (config["torch_git"], actual["torch_git"])
    for key in ("tensorfs_version", "extension_sha256", "shim_sha256"):
        assert config[key] and actual[key] == config[key], (key, config[key], actual[key])
    assert not torch.cuda.is_initialized(), "child initialized CUDA before allocator selection"
    torch.set_num_threads(1)
    if mode == "varena":
        vt.install(device=0)
    torch.cuda.set_device(0)
    if mode == "native":
        assert torch.cuda.get_allocator_backend() == "native", "baseline must be Torch native"
    stream = torch.cuda.default_stream(0)
    assert stream.cuda_stream == 0, "benchmark requires the legacy default stream"
    torch.cuda.set_stream(stream)
    properties = torch.cuda.get_device_properties(0)
    actual.update({
        "gpu": torch.cuda.get_device_name(0),
        "gpu_uuid": str(getattr(properties, "uuid", "unavailable")),
        "capability": list(torch.cuda.get_device_capability(0)),
        "driver_free_total": list(torch.cuda.mem_get_info()),
    })
    emit({"benchmark_provenance": actual, "mode": mode})

    # These are precisely the C++ entrypoints called by public caching_allocator_alloc/delete.
    # Selecting the device once avoids charging Python's device context manager on every pair.
    allocate = torch._C._cuda_cudaCachingAllocator_raw_alloc
    release = torch._C._cuda_cudaCachingAllocator_raw_delete
    handle = stream.cuda_stream

    def pairs(rounds):
        for _ in range(rounds):
            for size in SIZES:
                pointer = allocate(size, handle)
                release(pointer)

    def memory():
        if mode == "varena":
            row = vt.stats(0)
            assert row["mapped_bytes"] <= MAX_MAPPED and row["mapped_peak"] <= MAX_MAPPED, row
            return row
        row = {
            "allocated_bytes": torch.cuda.memory_allocated(0),
            "mapped_bytes": torch.cuda.memory_reserved(0),
            "mapped_peak": torch.cuda.max_memory_reserved(0),
        }
        assert row["mapped_bytes"] <= MAX_MAPPED and row["mapped_peak"] <= MAX_MAPPED, row
        return row

    gc.collect()
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    pairs(1024)  # one untimed warmup; leave the resulting cache/slabs intact for every sample
    torch.cuda.synchronize()
    warm = memory()
    actual["process_guard"] = json.loads(_ext._varena_process_guard_status())
    assert actual["process_guard"]["schema"] == "varena.process_guard/1"
    assert actual["process_guard"]["contract"] == "one_process/1"
    actual["pid"] = os.getpid()
    if mode == "varena":
        assert actual["process_guard"]["backend"] in ("kernel_wipe", "fallback_pid", "actual_pid")
    if actual["process_guard"]["backend"] == "kernel_wipe":
        assert actual["process_guard"]["marker"] == actual["pid"]
        assert actual["process_guard"]["thread_authenticated"]
    emit({"benchmark_warmup": warm, "mode": mode, "pairs": 1024 * len(SIZES)})
    samples = []
    pair_count = config["rounds"] * len(SIZES)
    enabled = gc.isenabled()
    gc.disable()
    try:
        for index in range(config["samples"]):
            torch.cuda.synchronize()
            before = memory()
            start = time.perf_counter_ns()
            pairs(config["rounds"])
            submitted = time.perf_counter_ns()
            torch.cuda.synchronize()
            completed = time.perf_counter_ns()
            after = memory()
            assert after["allocated_bytes"] == warm["allocated_bytes"], (warm, after)
            sample = {
                "index": index, "pairs": pair_count,
                "host_ns": submitted - start,
                "drain_ns": completed - submitted,
                "completion_ns": completed - start,
                "host_ns_per_pair": (submitted - start) / pair_count,
                "completion_ns_per_pair": (completed - start) / pair_count,
                "before": before, "after": after,
            }
            samples.append(sample)
            emit({"benchmark_sample": sample, "mode": mode})
    finally:
        if enabled:
            gc.enable()
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    final = memory()
    emit({"result": {
        "passed": True, "mode": mode, "provenance": actual,
        "sizes": list(SIZES), "stream": handle, "warmup_pairs": 1024 * len(SIZES),
        "samples": samples, "sample_count": len(samples), "pairs_per_sample": pair_count,
        "median_host_ns_per_pair": statistics.median(row["host_ns_per_pair"] for row in samples),
        "median_completion_ns_per_pair": statistics.median(row["completion_ns_per_pair"] for row in samples),
        "host_ns_per_pair_range": [min(row["host_ns_per_pair"] for row in samples), max(row["host_ns_per_pair"] for row in samples)],
        "managed_storage_limit_bytes": MAX_MAPPED, "maximum_live_request_bytes": max(SIZES),
        "timing_boundary": "host Python dispatch + Torch C++ raw allocator alloc/free; initialization, warmup, synchronization and telemetry excluded; completion/drain reported separately",
        "final": final,
    }})


if __name__ == "__main__":
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
