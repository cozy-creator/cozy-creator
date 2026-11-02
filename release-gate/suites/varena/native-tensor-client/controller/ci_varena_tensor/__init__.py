"""Ordinary Cozy custody for the unchanged typed-export GPU clients."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    source_sha: str
    script_source_sha: str
    gpu_uuid: str
    tensorfs_version: str
    runtime_version: str
    extension_sha256: str
    shim_sha256: str
    core_sha256: str
    std_name: str
    std_sha256: str
    tensorfs_files: dict[str, str]
    runtime_files: dict[str, str]
    scripts: dict[str, str]
    delay_cycles: int = 50_000_000
    timeout_s: float = 120.0


class Report(msgspec.Struct):
    passed: bool
    detail: str


def execute(payload, ctx, case):
    result = {"passed": False, "case": case, "input": msgspec.to_builtins(payload)}
    try:
        import hashlib
        import importlib.metadata

        import cozy_runtime
        import torch

        assert 0 < payload.timeout_s <= 180 and payload.delay_cycles > 0
        assert ctx.device.type == "cuda" and (ctx.device.index or 0) == 0

        def uuid_key(value):
            return (
                value.lower().removeprefix("gpu-").removeprefix("mig-").replace("-", "")
            )

        assert payload.gpu_uuid != "UNBOUND"
        actual_uuid = str(torch.cuda.get_device_properties(0).uuid)
        assert uuid_key(actual_uuid) == uuid_key(payload.gpu_uuid)
        assert importlib.metadata.version("cozy-runtime") == payload.runtime_version
        for name, expected in payload.runtime_files.items():
            assert (
                hashlib.sha256(
                    (Path(cozy_runtime.__file__).parent / name).read_bytes()
                ).hexdigest()
                == expected
            ), name
        result["parent"] = {
            "pid": os.getpid(),
            "request_id": ctx.request_id,
            "gpu_uuid": actual_uuid,
        }
        with tempfile.TemporaryDirectory(prefix="typed-export-client-") as folder:
            input_path = Path(folder) / "input.json"
            input_path.write_text(json.dumps(msgspec.to_builtins(payload)))
            command = [
                sys.executable,
                "-I",
                "-B",
                str(Path(__file__).with_name("runner.py")),
                str(input_path),
                case,
            ]
            child = subprocess.Popen(
                command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
            )
            pidfd = os.pidfd_open(child.pid)
            started = time.monotonic()
            stdout = stderr = ""
            try:
                while True:
                    ctx.raise_if_cancelled()
                    try:
                        stdout, stderr = child.communicate(timeout=0.25)
                        break
                    except subprocess.TimeoutExpired:
                        if time.monotonic() - started > payload.timeout_s:
                            raise TimeoutError(
                                "typed-export child exceeded its declared deadline"
                            )
            finally:
                if child.poll() is None:
                    signal.pidfd_send_signal(pidfd, signal.SIGTERM)
                    try:
                        stdout, stderr = child.communicate(timeout=2)
                    except subprocess.TimeoutExpired:
                        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                        stdout, stderr = child.communicate(timeout=5)
                result["child"] = {
                    "command": command,
                    "pid": child.pid,
                    "returncode": child.returncode,
                    "stdout": stdout,
                    "stderr": stderr,
                    "elapsed_s": time.monotonic() - started,
                    "exited": child.poll() is not None,
                }
                os.close(pidfd)
            assert child.returncode == 0, result["child"]
            rows = [
                json.loads(line) for line in stdout.splitlines() if line.startswith("{")
            ]
            assert rows and rows[-1].get("passed") is True, result["child"]
            result["native_result"] = rows[-1]
            assert rows[-1]["case"] == case and rows[-1]["pid"] == child.pid
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - retain complete failure before verdict
        result["error"], result["traceback"] = repr(error), traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def typed_exports(payload: Probe, ctx: Context) -> Report:
    return execute(payload, ctx, "typed_exports")


@app.entrypoint(accelerator=True)
def failed_tail(payload: Probe, ctx: Context) -> Report:
    return execute(payload, ctx, "failed_tail")
