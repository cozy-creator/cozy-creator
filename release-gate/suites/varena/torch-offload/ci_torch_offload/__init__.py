"""vt.offload through cozy, in a fresh process per run: stock Torch has no Varena in it."""

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import msgspec
import tensorfs.varena.torch as vt
import torch
from cozy_runtime.author import App, Context, Outputs
from cozy_runtime.media import File
from torch import nn

app = App()


class SDXL(msgspec.Struct, forbid_unknown_fields=True):
    mode: str  # stock, stock-cpu-offload, eager or graphs
    prompts: list[str]
    seed: int
    limit_bytes: int = 0  # The machine's --gpu-memory-limit: a fresh process doesn't inherit it.
    steps: int = 20
    size: int = 1024


class Adapter(msgspec.Struct, forbid_unknown_fields=True):
    label: str = ""


class Report(msgspec.Struct):
    passed: bool
    detail: str
    images: list[File] = msgspec.field(default_factory=list)


def _child(script: str, request: dict[str, str | int | list[str]], ctx: Context, out: Outputs) -> Report:
    env = {k: v for k, v in os.environ.items() if k != "TORCH_CUBLAS_WORKSPACE_CACHE"}
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "request.json"
        path.write_text(json.dumps({**request, "out": folder}))
        child = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__).with_name(script)), str(path)],
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
        assert child.stdout is not None
        lines: list[str] = []
        for line in child.stdout:
            lines.append(line.rstrip())
            ctx.raise_if_cancelled()
        code = child.wait()
        report = next((json.loads(line)["report"] for line in reversed(lines) if line.startswith('{"report"')), None)
        images: list[File] = [out.save_file(image, media_type="image/png") for image in sorted(Path(folder).glob("*.png"))]
    return Report(code == 0 and report is not None, json.dumps({"exit": code, "report": report, "tail": lines[-40:]}), images)


@app.entrypoint(accelerator=True)
def sdxl(request: SDXL, ctx: Context, out: Outputs) -> Report:
    return _child("sdxl.py", msgspec.to_builtins(request), ctx, out)


@app.entrypoint(accelerator=True)
def adapter(request: Adapter, ctx: Context, out: Outputs) -> Report:
    return _child("adapter.py", {}, ctx, out)


@app.entrypoint(accelerator=True)
def in_executor(request: Adapter, ctx: Context, out: Outputs) -> Report:
    """Runtime has installed the allocator and chosen an explicit stream: vt.offload must honor both."""
    device = torch.device("cuda", 0)
    stream = torch.cuda.current_stream(device)
    x = torch.randn(1, 8)
    model = nn.Sequential(nn.Linear(8, 8), nn.Linear(8, 8)).eval()
    with torch.inference_mode():
        want = model(x).to(device)
    vt.offload(model, device=0)
    with torch.inference_mode():
        got = [model(x.to(device)) for _ in range(6)][-1]
    passed = bool(torch.allclose(got, want, atol=1e-5)) and torch.cuda.current_stream(device) == stream
    return Report(passed, json.dumps({"stream": stream.cuda_stream > 2}))
