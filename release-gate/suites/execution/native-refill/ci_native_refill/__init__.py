"""Candidate wheels exercised through the normal local Cozy GPU lane."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    candidate_python: str
    seed: int = 103
    mode: str = "fits"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint
def gpu(payload: Probe, tel: Telemetry) -> Report:
    if payload.mode not in {
        "fits",
        "routes",
        "stream",
        "lora",
        "failure",
        "async",
        "async-cancel",
        "async-error",
        "prefetch",
        "prefetch-cancel",
        "prefetch-pressure",
        "prefetch-capture",
        "prefetch-replay",
        "prefetch-allocator",
    }:
        raise ValueError("unknown native refill gate mode")
    if not Path(payload.candidate_python).is_file():
        raise ValueError(
            "candidate_python must name an isolated prebuilt-wheel environment"
        )
    process = subprocess.Popen(
        [
            payload.candidate_python,
            "-I",
            str(
                Path(__file__).with_name(
                    "prefetch_probe.py"
                    if payload.mode.startswith("prefetch")
                    else "async_probe.py"
                    if payload.mode.startswith("async")
                    else "probe.py"
                )
            ),
            str(payload.seed),
            payload.mode,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    assert process.stdout is not None
    result = None
    for line in process.stdout:
        tel.log(line.rstrip())
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and "result" in row:
            result = row["result"]
    status = process.wait()
    if status or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"native refill child failed ({status}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))
