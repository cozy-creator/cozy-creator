"""Real allocator and public split operation in an isolated child of the installed SDK."""

import json
import subprocess
import sys
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "unsplittable"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    process = subprocess.Popen(
        [
            sys.executable,
            "-I",
            str(Path(__file__).with_name("exercise.py")),
            payload.mode,
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
                f"real split gate failed ({process.returncode}): {output[-12000:]}"
            )
        detail = json.loads(output.splitlines()[-1])
        assert detail["pid"] == process.pid
        return Report(True, json.dumps(detail, sort_keys=True))
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
