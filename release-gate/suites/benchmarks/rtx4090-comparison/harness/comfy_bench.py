# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""On the rental's GPU: start ComfyUI with its default flags, run one cell's JOBS one by one, stop it and return
results.tar.gz.

Per request: POST /prompt to execution_success (the save included), each node's segment, every progress event
(per-step times), nvidia-smi memory.used and utilisation every 0.5 s, ComfyUI's RSS and the host's MemAvailable.
Progress reaches the client as run.progress per step and a log line per event, so the laptop's 15-minute stall rule
sees it. Graphs are ComfyUI's own nodes as its templates wire them (workflow_templates 67984637), set to Cozy's
settings for the cell.
"""
import base64
import ctypes
import json
import os
import queue
import signal
import subprocess
import tarfile
import threading
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, FileAsset, Outputs, Telemetry

app = App()


class BenchRequest(msgspec.Struct):
    operation: str = "run"
    label: str = "sample"
    seed: int = 1
    prompt: str = ""
    require_warm: bool = False
    profile: bool = False


class BenchResult(msgspec.Struct):
    session: str = ""
    stopped: str = ""
    images: list[FileAsset] = msgspec.field(default_factory=list)
    trace: FileAsset | None = None
    archive: FileAsset | None = None


CELL = None  # @CELL@
JOBS = None  # @JOBS@
IMAGES = None  # @IMAGES@

B = Path("/var/tmp/bench4090-20261026")
COMFY = B / "ComfyUI"
PY = B / "comfy-env/bin/python"
KEEP_ENV = ("PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR", "USER", "CUDA_VISIBLE_DEVICES", "NVIDIA_VISIBLE_DEVICES",
            "NVIDIA_DRIVER_CAPABILITIES", "LD_LIBRARY_PATH")
DRIVER = None  # @DRIVER@
STEP_OBSERVER = None  # @STEP_OBSERVER@


OWNER_PID = os.getpid()
SESSION = None


def pdeathsig():
    ctypes.CDLL("libc.so.6", use_errno=True).prctl(1, signal.SIGKILL)
    if os.getppid() != OWNER_PID:
        os.kill(os.getpid(), signal.SIGKILL)


def terminate_owned(proc):
    if proc is None or proc.poll() is not None:
        return
    # Each child is started in its own session; this group belongs to this executor.
    os.killpg(proc.pid, signal.SIGTERM)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=10)


def stop_server():
    global SESSION
    state, SESSION = SESSION, None
    if state is None:
        return None
    assert state["owner_pid"] == os.getpid(), "server belongs to another executor birth"
    terminate_owned(state["proc"])
    state["log"].close()
    return {"pid": state["proc"].pid, "returncode": state["proc"].returncode,
            "start_ticks": state["start_ticks"], "stopped": True}


@app.entrypoint(accelerator=True)
def main(payload: BenchRequest, ctx: Context, tel: Telemetry, out: Outputs) -> BenchResult:
    """One ordinary CLI call and one image. Source identity stays fixed across seeds.

    operation=stop ends only the server held by this exact executor. A parent-death
    signal and process groups cover abnormal executor exit and cancellation. The
    owner still ends its bounded rental in finally; no second Cozy daemon is used.
    """
    operation = payload.operation
    label = payload.label
    seed = payload.seed
    prompt = payload.prompt
    require_warm = payload.require_warm
    profile = payload.profile
    global SESSION, OWNER_PID
    # The application may have been imported by the Runtime's pre-fork zygote.
    # Child parent-death checks must bind the actual calling executor, not that importer.
    OWNER_PID = os.getpid()
    assert SESSION is None or SESSION["owner_pid"] == OWNER_PID
    assert operation in ("run", "stop")
    assert label and all(c.isalnum() or c in "-_" for c in label)
    res = B / "results" / CELL / label
    res.mkdir(parents=True, exist_ok=False)
    if operation == "stop":
        stopped = stop_server()
        record = {"operation": "stop", "owner_pid": OWNER_PID, "server": stopped}
        path = res / "stopped.json"
        path.write_text(json.dumps(record, indent=2))
        archive = B / "results" / f"{CELL}-receipts.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(B / "results" / CELL, arcname=CELL)
        return BenchResult(stopped=json.dumps(record),
                           archive=out.save_file(archive, media_type="application/gzip"))
    assert len(JOBS) >= 1 and seed >= 0
    job = {**JOBS[0], "label": label, "seed": seed, "profile": profile}
    if prompt:
        job["prompt"] = prompt
    (res / "jobs.json").write_text(json.dumps([job], indent=2))
    smi = lambda query: subprocess.run(
        ["nvidia-smi", f"--query-{query}", "--format=csv,noheader"],
        capture_output=True, text=True).stdout.strip()
    rec = {"cell": CELL, "label": label, "started_unix_s": time.time(),
           "gpu_identity": SESSION["gpu_identity"] if SESSION is not None else smi("gpu=uuid,pci.bus_id,name,driver_version"),
           "wrapper_pid": os.getpid(), "required_warm": require_warm}
    drv = None
    healthy = False
    threads = []
    try:
        assert subprocess.check_output(["git", "-C", str(COMFY), "rev-parse", "HEAD"], text=True).strip() == "fb3befd168731438f88bc96e0920a538e8649878"
        assert json.loads((B / "setup.json").read_text())["ok"] is True
        match = json.loads((B / ("matched-sdxl.json" if CELL == "sdxl" else "matched-qwen.json")).read_text())
        assert match["matched"] is True and job["steps"] == 30
        assert rec["gpu_identity"] == match["gpu_identity"]
        rec["match"] = match
        if SESSION is not None and SESSION["proc"].poll() is not None:
            stop_server()
        rec["server_reused"] = SESSION is not None
        assert not require_warm or SESSION is not None, "warm server was retired; retain this failed sample"
        if SESSION is None:
            assert isinstance(STEP_OBSERVER, str) and STEP_OBSERVER
            observer = COMFY / "custom_nodes/cozy_benchmark_steps"
            observer.mkdir(parents=True, exist_ok=True)
            (observer / "__init__.py").write_text(STEP_OBSERVER)
            (COMFY / "input").mkdir(parents=True, exist_ok=True)
            for name, data in IMAGES.items():
                (COMFY / "input" / name).write_bytes(base64.b64decode(data))
            env = {k: v for k, v in os.environ.items() if k in KEEP_ENV}
            flags = ["--fp16-unet", "--fp16-text-enc", "--bf16-vae"] if CELL == "sdxl" else ["--bf16-unet", "--bf16-text-enc", match["comfy_vae_flag"]]
            cmd = [str(PY), "main.py", "--listen", "127.0.0.1", "--port", "8188", *flags]
            log = open(res / "comfy-server.log", "ab")
            started = time.time()
            proc = subprocess.Popen(cmd, cwd=COMFY, stdout=log, stderr=subprocess.STDOUT,
                                    env=env, preexec_fn=pdeathsig, start_new_session=True)
            SESSION = {"proc": proc, "log": log, "env": env, "started": started,
                       "owner_pid": OWNER_PID, "gpu_identity": rec["gpu_identity"],
                       "start_ticks": Path(f"/proc/{proc.pid}/stat").read_text().rsplit(")",1)[1].split()[19],
                       "command": cmd}
        state = SESSION
        rec.update(server_pid=state["proc"].pid, server_start_ticks=state["start_ticks"],
                   server_started_unix_s=state["started"], server_cmd=state["command"])
        (B / "driver.py").write_text(DRIVER)
        # Keep the existing driver, one job at a time, and return the image directly.
        drv = subprocess.Popen([str(PY), str(B / "driver.py"), CELL, str(res / "jobs.json"),
                                str(res), str(state["proc"].pid), str(state["started"])],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                               env=state["env"], preexec_fn=pdeathsig, start_new_session=True)
        lines = queue.Queue()
        def pump():
            for line in drv.stdout:
                lines.put(line)
            lines.put(None)
        pump_thread = threading.Thread(target=pump, daemon=True)
        pump_thread.start()
        threads.append(pump_thread)
        # Shared sampler_start.py observes both engines identically. Do not add a
        # second per-request nvidia-smi/proc observer only to the Comfy arm.
        while True:
            if ctx.cancelled:
                raise RuntimeError("benchmark cancelled; owned Comfy server will stop")
            try:
                line = lines.get(timeout=0.25)
            except queue.Empty:
                continue
            if line is None:
                break
            try:
                event = json.loads(line)
            except ValueError:
                tel.log(line.strip()[:1500])
                continue
            if event.get("ev") == "progress" and event.get("max"):
                tel.progress(min(.99, event["value"] / event["max"]),
                             stage=f"comfy {label} step {event['value']}/{event['max']}")
            else:
                tel.log(line.strip()[:1500])
        drv.wait(timeout=10)
        rec["driver_rc"] = drv.returncode
        result = json.loads((res / f"{label}.json").read_text())
        rec["request"] = result  # Retain history, timing and files even if qualification fails.
        assert drv.returncode == 0 and result["status"] == "execution_success", result
        assert result["qualified"] and result["confirmed_steps"] == job["steps"], result
        rec["qualified"] = True
        healthy = True
    except Exception as error:
        rec.update(qualified=False, error=repr(error))
    finally:
        terminate_owned(drv)
        for thread in threads:
            thread.join(timeout=3)
        if not healthy:
            rec["cleanup"] = stop_server()
    rec["seconds"] = time.time() - rec["started_unix_s"]
    rec["traffic_scope"] = "Shared external GPU/process sampler applies equally to both engines; this wrapper adds no per-request polling. Physical H2D/exposed gaps come from separate traces."
    (res / "session.json").write_text(json.dumps(rec, indent=2))
    # The session receipt rides run['result']['session']; the image(s) and optional trace are the
    # only binary outputs, matched downstream by media type (image/*, application/json), never by name.
    images = []
    trace = None
    if rec.get("request", {}).get("images"):
        images = [out.save_file(res / image["file"], media_type="image/png") for image in rec["request"]["images"]]
        if rec["request"].get("profile"):
            trace = out.save_file(res / rec["request"]["profile"]["copied_file"], media_type="application/json")
    archive = None
    if not healthy:
        evidence = res / "unqualified-receipts.tar.gz"
        with tarfile.open(evidence, "w:gz") as tar:
            for source in res.iterdir():
                if source != evidence:
                    tar.add(source, arcname=source.name)
        archive = out.save_file(evidence, media_type="application/gzip")
    return BenchResult(session=json.dumps(rec), images=images, trace=trace, archive=archive)
