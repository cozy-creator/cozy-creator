"""Run cases.py in fresh processes, stock Torch's allocator first for the cost comparison."""
import json
import subprocess
import sys
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    case: str = "all"
    seed: int = 138


class Report(msgspec.Struct):
    passed: bool
    detail: str


def child(mode, case, ctx):
    process = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__).with_name("cases.py")), mode, case],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    assert process.stdout is not None
    lines = []
    for line in process.stdout:
        lines.append(line.rstrip())
        ctx.raise_if_cancelled()
    code = process.wait()
    rows = [json.loads(line) for line in lines if line.startswith("{")]
    return {"exit": code, "rows": rows, "tail": lines[-30:] if code else []}


@app.entrypoint(accelerator=True)
def core(request: Request, ctx: Context) -> Report:
    native = child("native", "fast-path", ctx) if request.case in ("all", "fast-path") else None
    varena = child("varena", request.case, ctx)
    detail = json.dumps({"native": native, "varena": varena})
    if varena["exit"] != 0:
        raise RuntimeError(f"core rule failed: {detail[-4000:]}")
    return Report(True, detail)
