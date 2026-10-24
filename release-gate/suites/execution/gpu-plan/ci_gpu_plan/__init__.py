"""A same-machine candidate-wheel proof, scheduled by the normal Cozy GPU lane."""
import json
import subprocess
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    candidate_python: str
    seed: int = 103


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint
def repeat(payload: Probe, tel: Telemetry) -> Report:
    if not Path(payload.candidate_python).is_file():
        raise ValueError("candidate_python must name the isolated candidate environment")
    process = subprocess.Popen(
        [payload.candidate_python, "-I", str(Path(__file__).with_name("proof.py")), str(payload.seed)],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
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
        raise RuntimeError(f"GPU plan child failed ({status}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))
