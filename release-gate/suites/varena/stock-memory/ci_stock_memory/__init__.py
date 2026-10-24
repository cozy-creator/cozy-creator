"""A real residency cell. The child uses the candidate SDK, Torch and TensorFS; no mock backend.

The optional candidate_python is for a same-host local component proof without changing the
machine's installed SDK. Its versions and source hash are returned separately from the outer run.
Normal candidate-cohort proof leaves it empty and uses the installed executor's interpreter.
"""

import json
import subprocess
import sys
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    candidate_python: str = ""


class Report(msgspec.Struct):
    passed: bool
    detail: str


def run(payload: Probe, tel: Telemetry, device: str, mode: str = "stream") -> Report:
    python = payload.candidate_python or sys.executable
    if not Path(python).is_file():
        raise ValueError("candidate_python must name an existing interpreter on this machine")
    child = Path(__file__).with_name({"native": "native.py", "derived": "derived.py", "cancel": "cancel.py"}.get(mode, "probe.py"))
    with tel.stage(f"stock-memory-{device}"):
        process = subprocess.Popen(
            [python, "-I", str(child), device, str(payload.seed), mode],
            stdout=subprocess.PIPE,
            stdin=subprocess.PIPE if mode == "cancel" else None,
            stderr=subprocess.STDOUT,
            text=True,
        )
        result = None
        assert process.stdout is not None
        try:
            for line in process.stdout:
                tel.log(line.rstrip())
                if mode == "cancel":
                    tel.progress(0.0)  # the author API checks cancellation at this real work boundary
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if isinstance(row, dict) and "result" in row:
                    result = row["result"]
        finally:
            if mode == "cancel" and process.poll() is None:
                assert process.stdin is not None
                try:
                    process.stdin.write("stop\n")
                    process.stdin.flush()
                except BrokenPipeError:
                    pass
                for cleanup in process.stdout:
                    tel.log(cleanup.rstrip())
                process.wait()  # this owned child joins its stream and drops native pins
        exit_code = process.wait()
    if exit_code or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"stock-memory child failed ({exit_code}): {result}")
    return Report(passed=True, detail=json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=False)
def cpu(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cpu")


@app.entrypoint
def gpu(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cuda:0")


@app.entrypoint
def driver(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cuda:0", "driver")


@app.entrypoint
def native(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cuda:0", "native")


@app.entrypoint
def derived(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cuda:0", "derived")


@app.entrypoint
def cancel(payload: Probe, tel: Telemetry) -> Report:
    return run(payload, tel, "cuda:0", "cancel")
