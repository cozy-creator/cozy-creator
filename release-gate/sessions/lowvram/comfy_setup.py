# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.34", "msgspec>=0.19"]
# ///
"""On the rental (lowvram.sh): ComfyUI at a pinned commit in its own hash-locked venv (torch 2.14.1+cu130), and the
pinned model files for the session's models, from GitHub, PyPI, the PyTorch index and Hugging Face only. Downloads are
timed apart from the install. Returns the setup record."""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, Outputs, Telemetry

app = App()
B = Path("/var/tmp/lowvram")
COMFY = B / "ComfyUI"
COMFY_COMMIT = "fb3befd168731438f88bc96e0920a538e8649878"
REQUIREMENTS = None  # @REQUIREMENTS@
# model -> [(repo, revision, path in the repo, ComfyUI models folder)]
FILES = {
    "sdxl": [("stabilityai/stable-diffusion-xl-base-1.0", "462165984030d82259a11f4367a4eed129e94a7b",
              "sd_xl_base_1.0.safetensors", "checkpoints")],
    "qwen": [("Comfy-Org/Qwen-Image-2.1", "0217efd1d9c9452e84f6448903d6e21bfa171ffd",
              "diffusion_models/qwen_image_2.1_int8_convrot.safetensors", "diffusion_models"),
             ("Comfy-Org/Qwen-Image-2.1", "0217efd1d9c9452e84f6448903d6e21bfa171ffd",
              "text_encoders/qwen3vl_8b_int8_convrot.safetensors", "text_encoders"),
             ("Comfy-Org/Qwen-Image-2.1", "0217efd1d9c9452e84f6448903d6e21bfa171ffd",
              "vae/qwen_image_2.1_vae_bf16.safetensors", "vae")],
    "anima": [("circlestone-labs/Anima", "f973fc41ec7545364ac9776c2440285f43ff2a30",
               "split_files/diffusion_models/anima-base-v1.0.safetensors", "diffusion_models"),
              ("circlestone-labs/Anima", "f973fc41ec7545364ac9776c2440285f43ff2a30",
               "split_files/text_encoders/qwen_3_06b_base.safetensors", "text_encoders"),
              ("Comfy-Org/Qwen-Image", "1f12b17be14c89b026c51a91d67c32f84bb047bc",
               "split_files/vae/qwen_image_vae.safetensors", "vae")],
}


class SetupRequest(msgspec.Struct):
    models: list[str]


class SetupResult(msgspec.Struct):
    setup: str


def run(cmd: str, log: Path, tel: Telemetry, label: str, watch: Path | None = None, env: dict | None = None) -> dict:
    """One shell step into the log; while it runs, the bytes under `watch` every 30 s (the client's progress)."""
    t = time.time()
    with open(log, "ab") as f:
        f.write(f"\n$ {cmd}\n".encode())
        f.flush()
        p = subprocess.Popen(cmd, shell=True, stdout=f, stderr=subprocess.STDOUT, env=env)
        while p.poll() is None:
            time.sleep(min(30, 1 + time.time() - t))
            if watch is not None and watch.exists():
                n = sum(x.stat().st_size for x in watch.rglob("*") if x.is_file())
                tel.log(f"{label}: {n / 1e9:.2f} GB after {time.time() - t:.0f} s")
    return {"rc": p.returncode, "s": round(time.time() - t, 1)}


def text(cmd: str) -> str:
    return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout.strip()


def find_uv() -> str:
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
    rec: dict = {"started_unix_s": time.time(), "steps": {}, "comfy_commit": COMFY_COMMIT, "host": {
        "cpu": text("grep -m1 'model name' /proc/cpuinfo"), "nproc": os.cpu_count(),
        "meminfo": text("grep -E 'MemTotal|MemAvailable|SwapTotal' /proc/meminfo"), "df": text("df -h / /var/tmp"),
        "gpu": text("nvidia-smi --query-gpu=name,memory.total,driver_version,pcie.link.gen.max,pcie.link.width.max,"
                    "power.limit,clocks.max.sm --format=csv")}}
    try:
        uv = find_uv()
        env = dict(os.environ, UV_LINK_MODE="copy")
        if not (COMFY / ".git").exists():
            rec["steps"]["clone"] = run(
                f"git init -q {COMFY} && git -C {COMFY} remote add origin https://github.com/Comfy-Org/ComfyUI.git && "
                f"git -C {COMFY} fetch -q --depth 1 origin {COMFY_COMMIT} && git -C {COMFY} checkout -q FETCH_HEAD",
                log, tel, "clone")
        assert text(f"git -C {COMFY} rev-parse HEAD") == COMFY_COMMIT, "refuse a different ComfyUI checkout"
        assert isinstance(REQUIREMENTS, str) and REQUIREMENTS
        (B / "requirements.lock").write_text(REQUIREMENTS)
        rec["requirements_sha256"] = hashlib.sha256(REQUIREMENTS.encode()).hexdigest()
        rec["steps"]["venv"] = run(f"{uv} venv -q --allow-existing --python 3.12 {B}/comfy-env", log, tel, "venv", env=env)
        rec["steps"]["install"] = run(
            f"{uv} pip sync --python {B}/comfy-env/bin/python --require-hashes --index-strategy unsafe-best-match "
            f"{B}/requirements.lock", log, tel, "install", B / "comfy-env", env=env)
        assert rec["steps"]["install"]["rc"] == 0, "the locked ComfyUI environment did not install"
        rec["torch"] = text(f"{B}/comfy-env/bin/python -c 'import torch; print(torch.__version__, torch.version.cuda)'")
        assert rec["torch"].startswith("2.14.1+cu130"), rec["torch"]
        stage, t0, files = B / "hf", time.time(), {}
        for model in payload.models:
            for repo, rev, path, folder in FILES[model]:
                dst = COMFY / "models" / folder / Path(path).name
                if not dst.exists():
                    key = f"download {Path(path).name}"
                    rec["steps"][key] = run(f"{B}/comfy-env/bin/hf download {repo} {path} --revision {rev} "
                                            f"--local-dir {stage}/{repo}", log, tel, key, stage,
                                            env=dict(env, HF_HUB_DISABLE_TELEMETRY="1"))
                    dst.parent.mkdir(parents=True, exist_ok=True)
                    os.replace(stage / repo / path, dst)
                files[f"{folder}/{dst.name}"] = dst.stat().st_size
        rec["download_wall_s"] = round(time.time() - t0, 1)
        rec["files"] = files
        rec["download_gb"] = round(sum(files.values()) / 1e9, 2)
        shutil.rmtree(stage, ignore_errors=True)
        rec["ok"] = all(v["rc"] == 0 for v in rec["steps"].values())
    except Exception as e:  # noqa: BLE001 - the record says what failed
        rec["error"] = repr(e)
        rec["ok"] = False
        rec["log_tail"] = log.read_text()[-3000:] if log.exists() else ""
    rec["seconds"] = round(time.time() - rec["started_unix_s"], 1)
    tel.log(json.dumps({k: rec.get(k) for k in ("ok", "error", "torch", "download_gb", "download_wall_s", "seconds")}))
    if not rec["ok"]:
        raise RuntimeError(f"ComfyUI setup failed: {json.dumps(rec)[:3000]}")
    return SetupResult(json.dumps(rec))
