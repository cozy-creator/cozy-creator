"""Real installed SDK and CUDA, through the normal Cozy GPU entrypoint."""

import json
import subprocess
import sys
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "prefetch"
    seed: int = 1797


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    # The executor already owns an uncapped Memory. A real standalone consumer must set
    # its local cap before opening CUDA residency, using this same installed SDK.
    process = subprocess.Popen(
        [
            sys.executable,
            "-I",
            str(Path(__file__).with_name("exercise.py")),
            payload.mode,
            str(payload.seed),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        while True:
            ctx.raise_if_cancelled()
            try:
                output, _ = process.communicate(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                continue
        if process.returncode:
            raise RuntimeError(
                f"standalone memory gate failed ({process.returncode}): {output[-12000:]}"
            )
        result = json.loads(output.splitlines()[-1])
        assert result["child_pid"] == process.pid
        return Report(True, json.dumps(result, sort_keys=True))
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
