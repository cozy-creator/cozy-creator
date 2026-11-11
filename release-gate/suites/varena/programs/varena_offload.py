"""vt.offload's promises (`offload_adapter.py`) and SDXL through it beside stock Torch (`offload_sdxl.py`, modes stock, eager
and graphs), each in a fresh process, as varena/torch-offload ran them. One ROW each: `offload-adapter`, `offload-<mode>`.
Run by `release-gate/programs.sh --env diffusers` (xs-174)."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
ENV = {k: v for k, v in os.environ.items() if k != "TORCH_CUBLAS_WORKSPACE_CACHE"}
PROMPTS = ["a lighthouse on a basalt cliff at dusk, photo", "a fox in fresh snow, film still"]


def run(cell: str, script: str, request: dict[str, object]) -> None:
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "request.json"
        path.write_text(json.dumps({**request, "out": folder}))
        done = subprocess.run([sys.executable, "-I", "-B", str(HERE / script), str(path)], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, env=ENV)
    lines = done.stdout.splitlines()
    report = next((json.loads(line)["report"] for line in reversed(lines) if line.startswith('{"report"')), None)
    passed = done.returncode == 0 and report is not None and report.get("passed", True) is not False
    detail = report if passed else {"exit": done.returncode, "report": report, "tail": lines[-30:]}
    print("ROW " + json.dumps({"cell": cell, "result": "PASS" if passed else "FAIL", "detail": detail}, default=str), flush=True)


if __name__ == "__main__":
    run("offload-adapter", "offload_adapter.py", {})
    for index, mode in enumerate(("stock", "eager", "graphs")):
        run(f"offload-{mode}", "offload_sdxl.py", {"mode": mode, "prompts": PROMPTS, "seed": 1180 + index, "limit_bytes": 0,
                                                   "steps": 20, "size": 1024})
