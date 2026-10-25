"""A same-machine candidate-wheel proof, scheduled by the normal Cozy GPU lane."""

import json
import subprocess
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    candidate_python: str
    seed: int = 103
    mode: str = "repeat"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint
def external_pressure(ctx: Context, tel: Telemetry) -> Report:
    from .external_managed import exercise

    exercise(ctx, tel)
    return Report(False, "expected automatic pressure cancellation")


@app.entrypoint
def repeat(payload: Probe, tel: Telemetry) -> Report:
    if not Path(payload.candidate_python).is_file():
        raise ValueError(
            "candidate_python must name the isolated candidate environment"
        )
    probe = {
        "repeat": "proof.py",
        "consumer": "consumer.py",
        "retirement": "retirement.py",
        "pressure": "pressure.py",
        "lifecycle": "lifecycle.py",
        "lifecycle_cancel": "lifecycle.py",
        "lifecycle_running_cancel": "lifecycle.py",
    }.get(payload.mode)
    if probe is None:
        raise ValueError("unknown GPU-plan gate mode")
    controlled = payload.mode in (
        "lifecycle_cancel",
        "lifecycle_running_cancel",
    )
    process = subprocess.Popen(
        [
            payload.candidate_python,
            "-I",
            str(Path(__file__).with_name(probe)),
            str(payload.seed),
            payload.mode,
        ],
        stdout=subprocess.PIPE,
        stdin=subprocess.PIPE if controlled else None,
        stderr=subprocess.STDOUT,
        text=True,
    )
    assert process.stdout is not None
    result = None
    try:
        for line in process.stdout:
            tel.log(line.rstrip())
            if controlled:
                tel.progress(0.0, stage="pending-submit")
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if isinstance(row, dict) and "result" in row:
                result = row["result"]
    finally:
        if controlled and process.poll() is None:
            assert process.stdin is not None
            try:
                process.stdin.write("cancel\n")
                process.stdin.flush()
            except BrokenPipeError:
                pass
            for line in process.stdout:
                tel.log(line.rstrip())
            process.wait()
    status = process.wait()
    if status or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"GPU plan child failed ({status}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))
