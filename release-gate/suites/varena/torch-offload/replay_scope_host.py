"""Actual native host exclusion; no CUDA device, graph or weight Pin is created."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs import _ext
from tensorfs.varena import torch as vt


def capture_refused():
    try:
        _ext._VarenaAllocatorCapture(0)
    except RuntimeError as error:
        assert "captur" in str(error).lower(), str(error)
    else:
        raise AssertionError("native capture guard admitted a live replay scope")


def capture_allowed():
    guard = _ext._VarenaAllocatorCapture(0)
    guard.close()


capture_allowed()
with vt.replay_scope(0):
    capture_refused()
    with vt.replay_scope(0):
        capture_refused()
    capture_refused()
capture_allowed()
try:
    with vt.replay_scope(0):
        raise ValueError("actual caller exception")
except ValueError as error:
    assert str(error) == "actual caller exception"
capture_allowed()
held = _ext._VarenaAllocatorCapture(0)
try:
    try:
        with vt.replay_scope(0):
            raise AssertionError(
                "scope admitted during actual native capture exclusion"
            )
    except RuntimeError as error:
        assert "captur" in str(error).lower(), str(error)
finally:
    held.close()
capture_allowed()
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "native_host_exclusion": "pass",
            "nested_scope_preserves_outer": True,
            "finally_releases_on_exception": True,
            "native_capture_guard_refuses_replay_and_reverse": True,
            "cuda": False,
            "weight_pins_or_gpu_completion": "not exercised",
        }
    )
)
