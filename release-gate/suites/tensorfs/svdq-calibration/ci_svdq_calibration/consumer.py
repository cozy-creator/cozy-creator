"""Existing SVDQ readiness and fallback evidence for the real native consumer gate."""

from __future__ import annotations

import hashlib
import inspect
from functools import partial
from typing import Any

import torch
from cozy_runtime._build_provenance import COMMIT
from cozy_runtime.internal import int8_linear, kernel_ready, svdq_linear, verify


def ready(layer: Any) -> dict[str, Any]:
    assert layer.provider == "cozy.svdq-nvfp4.native-leaf/1"
    assert layer.encoding == "svdq.nvfp4"
    device = layer.bias.device
    assert device.type == "cuda" and layer.out_dtype == torch.bfloat16
    assert not torch.is_autocast_enabled("cuda")
    capability = torch.cuda.get_device_capability(device)
    assert capability == (10, 0), capability
    slot = f"{svdq_linear.KERNELS.name} cuda:{device.index or 0}"
    # The existing blocking readiness path waits for the ordinary build and self-check.
    # It never replaces the launcher or bypasses the model's normal forward method.
    launch = kernel_ready.wait(slot, partial(svdq_linear._opened, torch, device))
    verdict = verify.verified(
        svdq_linear.KERNELS.name,
        launch.key,
        100,
        partial(svdq_linear.probe, torch, device, launch),
    )
    assert verdict.ok, verdict.detail
    selected = svdq_linear.serving(device.index or 0)
    assert selected == svdq_linear.TRITON, selected
    assert not kernel_ready.failures(svdq_linear.KERNELS.name)
    _no_fallback()
    return {
        "selected_before": selected,
        "build_key": launch.key,
        "probe_passed": verdict.ok,
        "probe_detail": verdict.detail,
        "runtime_commit": COMMIT,
        "torch": str(torch.__version__),
        "cuda": torch.version.cuda,
        "consumer_source_sha256": hashlib.sha256(
            inspect.getsource(svdq_linear).encode()
        ).hexdigest(),
    }


def _no_fallback() -> None:
    warnings = [
        (message, fields)
        for message, fields in int8_linear.WARNINGS
        if fields.get("op") == svdq_linear.TRITON
    ]
    assert not warnings, warnings


def consumed(layer: Any, result: torch.Tensor, evidence: dict[str, Any]) -> None:
    # linear()'s only eager branches are an unavailable launcher, incompatible dtype,
    # or a launch exception recorded by int8_linear.failed. Check all three boundaries.
    torch.cuda.synchronize(layer.bias.device)
    selected = svdq_linear.serving(layer.bias.device.index or 0)
    assert selected == svdq_linear.TRITON, selected
    assert result.is_cuda and result.dtype == torch.bfloat16
    assert not kernel_ready.failures(svdq_linear.KERNELS.name)
    _no_fallback()
    evidence.update(selected_after=selected, launch_fallback=False, synchronized=True)
