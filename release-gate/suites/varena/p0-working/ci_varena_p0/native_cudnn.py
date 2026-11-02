"""Native Torch/cuDNN lifetime control: no Varena allocator installation or handle destruction."""
from __future__ import annotations

import gc
import hashlib
from importlib.metadata import version
import json
from pathlib import Path
import platform
import sys

import tensorfs
from tensorfs import _ext
import torch
import torch.nn.functional as functional

MIB = 1 << 20


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main(config):
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
    }
    for key in ("tensorfs_version", "extension_sha256", "shim_sha256"):
        assert config[key] and actual[key] == config[key], (key, config[key], actual[key])
    assert actual["torch"] == config["torch_version"], actual
    assert actual["torch_git"] == config["torch_git"], actual
    assert actual["torch_distribution"] == "2.14.0" and actual["torch_cuda"] == "13.0", actual
    assert not torch.cuda.is_initialized(), "native control initialized CUDA before its boundary"
    assert torch.cuda.get_allocator_backend() == "native", "control must select stock Torch native"
    torch.set_num_threads(1)
    torch.cuda.set_device(0)
    torch.manual_seed(config["seed"])
    actual.update({
        "gpu": torch.cuda.get_device_name(0),
        "capability": list(torch.cuda.get_device_capability(0)),
        "allocator_backend": torch.cuda.get_allocator_backend(),
        "cudnn_version": torch.backends.cudnn.version(),
    })
    assert actual["allocator_backend"] == "native"
    assert torch.backends.cudnn.enabled and not torch.backends.cudnn.benchmark
    emit({"native_cudnn_provenance": actual})

    generator = torch.Generator(device="cpu").manual_seed(config["seed"])
    host_x = torch.randn((1, 32, 128, 128), generator=generator) * 0.25
    host_w = torch.randn((32, 32, 3, 3), generator=generator) * 0.125
    x, weight = host_x.cuda(), host_w.cuda()
    with torch.no_grad():
        output = functional.conv2d(x, weight, padding=1)
        host_output = output.cpu()
    assert list(host_output.shape) == [1, 32, 128, 128]
    assert bool(torch.isfinite(host_output).all())
    output_sha = hashlib.sha256(host_output.numpy().tobytes()).hexdigest()
    peak = {
        "allocated_bytes": torch.cuda.max_memory_allocated(0),
        "reserved_bytes": torch.cuda.max_memory_reserved(0),
    }
    assert peak["allocated_bytes"] <= 256 * MIB and peak["reserved_bytes"] <= 256 * MIB, peak
    del x, weight, output, host_output, host_x, host_w, generator
    gc.collect()
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    host_before = torch.cuda.host_memory_stats()
    torch.accelerator.empty_host_cache()
    host_after = torch.cuda.host_memory_stats()
    managed = {
        "allocated_bytes": torch.cuda.memory_allocated(0),
        "reserved_bytes": torch.cuda.memory_reserved(0),
    }
    assert managed["allocated_bytes"] == managed["reserved_bytes"] == 0, managed
    assert host_after["active_bytes.current"] == host_after["allocated_bytes.current"] == 0, host_after
    emit({"result": {
        "passed": True, "control_only": True, "torch_managed_cleanup_passed": True,
        "scope": "native Torch/cuDNN lifetime control; sanitizer process-exit verdict remains separate",
        "provenance": actual, "shape": [1, 32, 128, 128], "weight_shape": [32, 32, 3, 3],
        "padding": 1, "dtype": "float32", "seed": config["seed"],
        "output_sha256": output_sha, "peak": peak, "final": managed,
        "host_cache": {"before": host_before, "after": host_after},
    }})


if __name__ == "__main__":
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
