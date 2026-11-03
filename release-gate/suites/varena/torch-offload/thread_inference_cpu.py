"""Actual inference Tensor mutation across worker threads; reproduced GPU26595."""

from __future__ import annotations

import json
import threading
from contextlib import nullcontext
from pathlib import Path

import torch

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


def copy_on_worker(destination, source, *, enable_inference):
    report = {}

    def worker():
        try:
            with torch.inference_mode() if enable_inference else nullcontext():
                report["inference_mode"] = torch.is_inference_mode_enabled()
                destination.copy_(source)
        except RuntimeError as error:
            report["error"] = str(error)

    task = threading.Thread(target=worker)
    task.start()
    task.join()
    return report


with torch.inference_mode():
    source = torch.arange(32, dtype=torch.uint8)
    failed_destination = torch.empty_like(source)
    destination = torch.empty_like(source)
    assert torch.is_inference(destination)
    # The caller's mode is deliberately still active when both threads start.
    red = copy_on_worker(failed_destination, source, enable_inference=False)
    assert red["inference_mode"] is False
    assert "Inplace update to inference tensor outside InferenceMode" in red["error"]
    green = copy_on_worker(destination, source, enable_inference=True)
    assert green == {"inference_mode": True}, green
    assert torch.equal(destination, source)
    assert torch.is_inference_mode_enabled()
assert not torch.is_inference_mode_enabled()
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "caller_inference_mode_not_inherited_by_thread": red,
            "explicit_worker_inference_mode_copy": green,
            "exact_values_and_caller_context_preserved": True,
            "gpu_capture_stream_pool_or_native_completion": "not exercised",
        }
    )
)
