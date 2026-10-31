"""The ordinary Cozy call owns a fresh candidate child; the installed Runtime remains visible."""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

import msgspec
from cozy_runtime.author import App, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    seed: int = 138
    case: str = "all"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, tel: Telemetry) -> Report:
    config = msgspec.json.encode(payload).decode()
    child = Path(__file__).with_name("probe.py")
    result = None
    with tel.stage("varena-p0"):
        process = subprocess.Popen(
            [sys.executable, "-I", str(child), config],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        assert process.stdout is not None
        try:
            for line in process.stdout:
                tel.log(line.rstrip())
                tel.progress(0.0)
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict) and "result" in record:
                    result = record["result"]
            code = process.wait()
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
    if code or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"varena P0 child failed ({code}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))
