# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""On the rental: ComfyUI at a pinned commit in its own venv, and the pinned Comfy-Org 8-bit files for every
ComfyUI cell. Downloads are timed apart from the install. Nothing comes from the laptop: GitHub, PyPI, the
pytorch index and Hugging Face only. Returns setup.json and the logs.
"""
import json
import hashlib
import signal
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, FileAsset, Outputs, Telemetry

app = App()


class SetupRequest(msgspec.Struct):
    pass


class SetupResult(msgspec.Struct):
    setup: str
    logs: list[FileAsset]


B = Path("/var/tmp/bench4090-20261026")
COMFY = B / "ComfyUI"
COMFY_COMMIT = "fb3befd168731438f88bc96e0920a538e8649878"  # @COMFY_COMMIT@
TORCH = "2.14.1"  # @TORCH@
# (repo, revision, file in the repo, ComfyUI models folder)
FILES = [("stabilityai/stable-diffusion-xl-base-1.0", "462165984030d82259a11f4367a4eed129e94a7b", "sd_xl_base_1.0.safetensors", "checkpoints")]

IDENTITY = None  # @IDENTITY@
REQUIREMENTS = None  # @REQUIREMENTS@
WANT = None  # @WANT@
FILES = [f for f in FILES if WANT is None or Path(f[2]).name in WANT]


def run(cmd, log, **kw):
    t = time.time()
    with open(log, "ab") as f:
        f.write(f"\n$ {cmd}\n".encode())
        f.flush()
        rc = subprocess.run(cmd, shell=True, stdout=f, stderr=subprocess.STDOUT, **kw).returncode
    return {"rc": rc, "s": round(time.time() - t, 1)}


def run_watched(cmd, log, ctx, tel, label, watch, **kw):
    """`run`, logging the bytes under `watch` every 30 s while they grow: the client's progress signal."""
    t = time.time()
    with open(log, "ab") as f:
        f.write(f"\n$ {cmd}\n".encode())
        f.flush()
        p = subprocess.Popen(cmd, shell=True, stdout=f, stderr=subprocess.STDOUT, start_new_session=True, **kw)
        last, observed = -1, 0.0
        try:
            while p.poll() is None:
                ctx.raise_if_cancelled()
                now = time.monotonic()
                if now - observed >= 5:
                    observed = now
                    n = sum(x.stat().st_size for x in watch.rglob("*") if x.is_file())
                    if n != last:
                        last = n
                        tel.log(f"{label}: {n / 1e9:.1f} GB after {time.time() - t:.0f} s")
                time.sleep(1)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGTERM)
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait()
    return {"rc": p.returncode, "s": round(time.time() - t, 1)}


def text(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout.strip()


def find_uv():
    for c in [shutil.which("uv"), "/usr/local/bin/uv", "/opt/cozy/bin/uv", "/root/.local/bin/uv"]:
        if c and os.path.exists(c):
            return c
    subprocess.run([sys.executable, "-m", "venv", str(B / "uvtool")], check=True)
    subprocess.run([str(B / "uvtool/bin/pip"), "install", "-q", "uv"], check=True)
    return str(B / "uvtool/bin/uv")


@app.entrypoint(accelerator=False)
def main(payload: SetupRequest, ctx: Context, tel: Telemetry, out: Outputs) -> SetupResult:
    B.mkdir(parents=True, exist_ok=True)
    log = B / "setup.log"
    rec = {"started_unix_s": time.time(), "steps": {}, "comfy_commit_wanted": COMFY_COMMIT, "torch_wanted": TORCH}
    rec["host"] = {
        "cpu": text("grep -m1 'model name' /proc/cpuinfo"),
        "nproc": os.cpu_count(),
        "meminfo": text("grep -E 'MemTotal|MemAvailable' /proc/meminfo"),
        "df": text("df -h / /root 2>/dev/null"),
        "gpu": text("nvidia-smi --query-gpu=name,memory.total,driver_version,pcie.link.gen.current,pcie.link.width.current,"
                    "power.limit,clocks.max.sm --format=csv"),
    }
    try:
        uv = find_uv()
        env = dict(os.environ, UV_LINK_MODE="copy")
        if not (COMFY / ".git").exists():
            rec["steps"]["clone"] = run(
                f"git init -q {COMFY} && git -C {COMFY} remote add origin https://github.com/Comfy-Org/ComfyUI.git && "
                f"git -C {COMFY} fetch -q --depth 1 origin {COMFY_COMMIT} && git -C {COMFY} checkout -q FETCH_HEAD", log)
        rec["comfy_commit"] = text(f"git -C {COMFY} rev-parse HEAD")
        assert rec["comfy_commit"] == COMFY_COMMIT, "refuse a different existing Comfy checkout"
        rec["steps"]["venv"] = run(f"{uv} venv -q --allow-existing --python 3.12 {B}/comfy-env", log, env=env)
        assert isinstance(REQUIREMENTS, str) and REQUIREMENTS
        (B / "requirements.lock").write_text(REQUIREMENTS)
        rec["requirements_sha256"] = hashlib.sha256(REQUIREMENTS.encode()).hexdigest()
        rec["steps"]["install"] = run_watched(
            f"{uv} pip sync --python {B}/comfy-env/bin/python --require-hashes {B}/requirements.lock",
            log, ctx, tel, "install", B / "comfy-env", env=env)
        assert rec["steps"]["install"]["rc"] == 0, "Comfy locked environment install failed"
        run(f"VIRTUAL_ENV={B}/comfy-env {uv} pip freeze > {B}/comfy-env-freeze.txt", log, env=env)
        rec["torch"] = text(f"{B}/comfy-env/bin/python -c 'import torch; print(torch.__version__, torch.version.cuda)'")
        assert rec["torch"] == "2.14.1+cu130 13.0", rec["torch"]
        stage = B / "hf"
        tel.log("venv ready; downloading the pinned files (timed alone, before the install)")
        t0 = time.time()
        for repo, rev, path, _ in FILES:
            key = f"download {repo}/{Path(path).name}"
            rec["steps"][key] = run_watched(
                f"{B}/comfy-env/bin/hf download {repo} {path} --revision {rev} --local-dir {stage}/{repo}",
                log, ctx, tel, key, stage, env=dict(env, HF_HUB_DISABLE_TELEMETRY="1"))
        rec["download_wall_s"] = round(time.time() - t0, 1)
        files = {}
        for repo, _, path, folder in FILES:
            src, dst = stage / repo / path, COMFY / "models" / folder / Path(path).name
            dst.parent.mkdir(parents=True, exist_ok=True)
            if src.exists():
                os.replace(src, dst)
            if dst.exists():
                files[f"{folder}/{dst.name}"] = dst.stat().st_size
        rec["files"] = files
        rec["download_bytes"] = sum(files.values())
        rec["download_gbps"] = round(rec["download_bytes"] / 1e9 / max(rec["download_wall_s"], 0.1), 3)
        tel.log(f"downloads {rec['download_bytes'] / 1e9:.1f} GB in {rec['download_wall_s']} s")
        rec["file_sha256"] = {}
        for filename in files:
            with (COMFY / "models" / filename).open("rb") as stream:
                rec["file_sha256"][filename] = hashlib.file_digest(stream, "sha256").hexdigest()
        if any(Path(item[2]).name == "sd_xl_base_1.0.safetensors" for item in FILES):
            rec["steps"]["configs"] = run(f"{B}/comfy-env/bin/hf download stabilityai/stable-diffusion-xl-base-1.0 --revision 462165984030d82259a11f4367a4eed129e94a7b --include '*.json' '*.txt' --local-dir {B}/sdxl-config", log, env=env)
            (B / "identity.py").write_text(IDENTITY)
            rec["steps"]["identity"] = run(f"{B}/comfy-env/bin/python {B}/identity.py", log, env=env)
        tel.log(f"install {rec['steps']['install']}, torch {rec['torch']}")
        rec["ok"] = all(v["rc"] == 0 for v in rec["steps"].values()) and len(files) == len(FILES)
    except Exception as e:  # noqa: BLE001 - return what we have
        rec["error"] = repr(e)
        rec["ok"] = False
    rec["seconds"] = round(time.time() - rec["started_unix_s"], 1)
    rec["df_after"] = text("df -h / 2>/dev/null")
    (B / "setup.json").write_text(json.dumps(rec, indent=1))
    # Receipt rides run['result']['setup']; only the aux logs are binary outputs.
    logs = [out.save_file(f, media_type="text/plain")
            for f in [B / "setup.log", B / "comfy-env-freeze.txt", B / "comfy-identity.json"] if f.exists()]
    return SetupResult(setup=json.dumps(rec), logs=logs)
