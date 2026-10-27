"""Each captured node class inside a native conditional body, one child process per variant: a
native fault fails this run, not the executor or the gate."""

import json
import subprocess
import sys
import time
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()
CHILD = Path(__file__).with_name("child.py")
PREFIX = "[conditional-body] "
VARIANTS = ("scalar", "memset", "memset32", "memcpy", "reduce", "parallel", "triton", "triton_pdl",
            "cublas", "cudnn", "sdpa", "mix")


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 331


class Report(msgspec.Struct):
    passed: bool
    detail: str


def variant(name: str, seed: int, ctx: Context) -> dict[str, object]:
    started = time.monotonic()
    process = subprocess.Popen([sys.executable, "-I", str(CHILD), name, str(seed)],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        while True:
            ctx.raise_if_cancelled()
            try:
                output, _ = process.communicate(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                continue
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    rows = [json.loads(line[len(PREFIX):]) for line in output.splitlines() if line.startswith(PREFIX)]
    last = rows[-1] if rows else {"stage": "start"}
    code = process.returncode
    if code < 0:
        outcome = f"signal {-code} after {last['stage']}"
    elif code:
        outcome = f"exit {code} after {last['stage']}: {output[-1500:]}"
    elif last["stage"] != "executed":
        outcome = f"no result after {last['stage']}"
    elif not last["passed"]:
        outcome = f"wrong values {last['checks']}"
    else:
        outcome = "pass"
    return {"outcome": outcome, "seconds": round(time.monotonic() - started, 1)}


def gpus() -> str:
    try:
        found = subprocess.run(["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv,noheader"],
                               capture_output=True, text=True)
        return found.stdout.strip() or found.stderr.strip()
    except OSError as error:
        return f"nvidia-smi: {error}"


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    assert ctx.device.type == "cuda"
    rows = {}
    for index, name in enumerate(VARIANTS):
        with tel.stage(name, overall_range=(index / len(VARIANTS), (index + 1) / len(VARIANTS))):
            rows[name] = variant(name, payload.seed, ctx)
    failed = {name: row["outcome"] for name, row in rows.items() if row["outcome"] != "pass"}
    if failed:
        raise RuntimeError(f"conditional body on {gpus()}: " + "; ".join(f"{k} {v}" for k, v in failed.items()))
    return Report(True, json.dumps({"gpu": gpus(), "variants": rows}, sort_keys=True))
