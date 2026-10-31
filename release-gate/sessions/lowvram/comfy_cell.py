# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.34", "msgspec>=0.19"]
# ///
"""On the rental (lowvram.sh): a fresh ComfyUI server (default flags) on GPU 0, the jobs one by one over its API,
then the server stops. Per request: submit to execution_success (the save included), each node's segment, the
sampler's step times, and from the shared NVML sampler (tool.py) the H2D bytes and the peak memory used. Returns the
per-request records and the images. The function itself holds no GPU: ComfyUI is its child on GPU 0."""
import json
import os
import signal
import subprocess
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, File, Outputs, Telemetry

app = App()
B = Path("/var/tmp/lowvram")
COMFY = B / "ComfyUI"
PY = B / "comfy-env/bin/python"

DRIVER = r'''
import asyncio, json, os, shutil, sys, time, urllib.request, uuid
from pathlib import Path
import aiohttp
URL = "http://127.0.0.1:8188"
COMFY = Path(sys.argv[3])

def node(kind, **inputs):
    return {"class_type": kind, "inputs": inputs}

def save(images, job):
    return node("SaveImage", images=images, filename_prefix=f"bench/{job['label']}")

def sdxl(job):  # ComfyUI's default SDXL graph (fp16 base, its own VAE)
    return {"1": node("CheckpointLoaderSimple", ckpt_name="sd_xl_base_1.0.safetensors"),
            "2": node("CLIPTextEncode", clip=["1", 1], text=job["prompt"]),
            "3": node("CLIPTextEncode", clip=["1", 1], text=""),
            "4": node("EmptyLatentImage", width=1024, height=1024, batch_size=1),
            "5": node("KSampler", model=["1", 0], seed=job["seed"], steps=job["steps"], cfg=job["guidance"], sampler_name="euler",
                      scheduler="normal", positive=["2", 0], negative=["3", 0], latent_image=["4", 0], denoise=1.0),
            "6": node("VAEDecode", samples=["5", 0], vae=["1", 2]),
            "7": save(["6", 0], job)}

def qwen(job):  # image_qwen_image_2_1_t2i, prompt enhancement off, int8 ConvRot DiT and text encoder
    return {"1": node("UNETLoader", unet_name="qwen_image_2.1_int8_convrot.safetensors", weight_dtype="default"),
            "2": node("QwenImage21Cache", model=["1", 0], device="auto", dtype="default"),
            "3": node("CLIPLoader", clip_name="qwen3vl_8b_int8_convrot.safetensors", type="qwen_image", device="default"),
            "4": node("VAELoader", vae_name="qwen_image_2.1_vae_bf16.safetensors"),
            "5": node("TextEncodeQwenImage21", clip=["3", 0], prompt=job["prompt"], negative_prompt="", resolution=1024),
            "6": node("EmptyLatentImage", width=1024, height=1024, batch_size=1),
            "7": node("KSampler", model=["2", 0], seed=job["seed"], steps=job["steps"], cfg=1.0, sampler_name="euler",
                      scheduler="simple", positive=["5", 0], negative=["5", 1], latent_image=["6", 0], denoise=1.0),
            "8": node("VAEDecode", samples=["7", 0], vae=["4", 0]),
            "9": save(["8", 0], job)}

def anima(job):  # blueprints/text_to_image_anima_base_1_0: er_sde/simple, the template's negative
    return {"1": node("UNETLoader", unet_name="anima-base-v1.0.safetensors", weight_dtype="default"),
            "2": node("CLIPLoader", clip_name="qwen_3_06b_base.safetensors", type="stable_diffusion", device="default"),
            "3": node("VAELoader", vae_name="qwen_image_vae.safetensors"),
            "4": node("CLIPTextEncode", clip=["2", 0], text=job["prompt"]),
            "5": node("CLIPTextEncode", clip=["2", 0], text=job["negative"]),
            "6": node("EmptyLatentImage", width=1024, height=1024, batch_size=1),
            "7": node("KSampler", model=["1", 0], seed=job["seed"], steps=job["steps"], cfg=job["guidance"], sampler_name="er_sde",
                      scheduler="simple", positive=["4", 0], negative=["5", 0], latent_image=["6", 0], denoise=1.0),
            "8": node("VAEDecode", samples=["7", 0], vae=["3", 0]),
            "9": save(["8", 0], job)}

GRAPHS = {"sdxl": sdxl, "qwen": qwen, "anima": anima}

def emit(**kw):
    print(json.dumps(kw), flush=True)

async def run_one(session, g, job, out):
    client, events, label = uuid.uuid4().hex, [], job["label"]
    async with session.ws_connect(f"ws://127.0.0.1:8188/ws?clientId={client}", max_msg_size=0, heartbeat=30) as ws:
        t0 = time.time()
        async with session.post(URL + "/prompt", json={"prompt": g, "client_id": client}) as r:
            body = await r.json()
            if r.status != 200:
                return {"label": label, "status": "rejected", "body": body, "submit_unix_s": t0, "total_s": 0}
        pid = body["prompt_id"]
        while True:
            msg = await ws.receive()
            now = time.time() - t0
            if msg.type == aiohttp.WSMsgType.BINARY:
                continue
            if msg.type != aiohttp.WSMsgType.TEXT:
                events.append({"t": round(now, 4), "type": "ws_closed", "data": {}})
                break
            m = json.loads(msg.data)
            data = m.get("data") or {}
            if m["type"] in ("status", "progress_state", "crystools.monitor") or data.get("prompt_id") not in (None, pid):
                continue
            events.append({"t": round(now, 4), "type": m["type"], "data": data})
            if m["type"] == "progress":
                emit(ev="progress", label=label, value=data.get("value"), max=data.get("max"), t=round(now, 2))
            elif m["type"] in ("execution_success", "execution_error", "execution_interrupted"):
                break
    total = events[-1]["t"] if events else 0
    with urllib.request.urlopen(f"{URL}/history/{pid}") as r:
        history = json.loads(r.read()).get(pid, {})
    files = []
    for node_out in history.get("outputs", {}).values():
        for items in node_out.values():
            for item in items if isinstance(items, list) else []:
                if isinstance(item, dict) and "filename" in item:
                    src = COMFY / item.get("type", "output") / item.get("subfolder", "") / item["filename"]
                    if src.exists():
                        dst = out / f"{label}{src.suffix}"
                        shutil.copy2(src, dst)
                        files.append(dst.name)
    segments, cur = [], None
    for e in events + [{"type": "end", "t": total, "data": {}}]:
        if e["type"] in ("executing", "execution_success", "execution_error", "execution_interrupted", "ws_closed", "end"):
            if cur:
                segments.append([cur[0], round(e["t"] - cur[1], 3)])
            n = e["data"].get("node") if e["type"] == "executing" else None
            cur = [g.get(n, {}).get("class_type"), e["t"]] if n else None
    progress = [e["t"] for e in events if e["type"] == "progress"]
    return {"label": label, "status": events[-1]["type"] if events else "none", "total_s": total, "submit_unix_s": t0,
            "segments": segments, "step_s": [round(b - a, 4) for a, b in zip(progress, progress[1:])],
            "first_step_at_s": progress[0] if progress else None, "files": files,
            "error": [e["data"] for e in events if e["type"] == "execution_error"]}

async def main():
    jobs, out, spid, t0 = json.loads(Path(sys.argv[1]).read_text()), Path(sys.argv[2]), int(sys.argv[4]), float(sys.argv[5])
    alive = lambda: os.path.exists(f"/proc/{spid}") and open(f"/proc/{spid}/stat").read().split()[2] != "Z"
    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=None)) as session:
        while True:
            if not alive():
                emit(ev="server_exited_during_start")
                return
            try:
                async with session.get(URL + "/system_stats") as r:
                    stats = await r.json()
                break
            except aiohttp.ClientError:
                await asyncio.sleep(0.25)
        emit(ev="server_ready", s=round(time.time() - t0, 2), devices=stats.get("devices"))
        for job in jobs:
            if not alive():
                emit(ev="server_dead_before", label=job["label"])
                return
            try:
                res = await run_one(session, GRAPHS[job["cell"]](job), job, out)
            except Exception as e:
                res = {"label": job["label"], "status": "driver_error", "error": repr(e), "total_s": 0, "submit_unix_s": time.time()}
            (out / f"{job['label']}.json").write_text(json.dumps(res, indent=1))
            emit(ev="job_done", label=job["label"], status=res["status"], total_s=res.get("total_s"), error=str(res.get("error"))[:600])

asyncio.run(main())
'''


class CellRequest(msgspec.Struct):
    jobs: str  # a JSON list of {label, cell, prompt, seed, steps, guidance[, negative]}


class CellResult(msgspec.Struct):
    session: str
    images: list[File] = msgspec.field(default_factory=list)


def nvml_rows() -> list[list[float]]:
    try:
        return [[float(x) for x in line.split(",")] for line in (B / "nvml.csv").read_text().splitlines()[1:] if line.count(",") == 2]
    except (OSError, ValueError):
        return []


def window(rows: list[list[float]], a: float, b: float) -> dict:
    win = [r for r in rows if a <= r[0] <= b]
    if len(win) < 2:
        return {}
    return {"h2d_gb": round((win[-1][1] - win[0][1]) / 1e9, 3), "peak_used_mib": int(max(r[2] for r in win)),
            "used_at_start_mib": int(win[0][2])}


@app.entrypoint(accelerator=False)
def main(payload: CellRequest, ctx: Context, tel: Telemetry, out: Outputs) -> CellResult:
    jobs = json.loads(payload.jobs)
    name = jobs[0]["label"].rsplit("-", 1)[0]
    res = B / "comfy-results" / name
    res.mkdir(parents=True, exist_ok=True)
    (res / "jobs.json").write_text(json.dumps(jobs))
    (B / "comfy-driver.py").write_text(DRIVER)
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "USER", "LD_LIBRARY_PATH",
                                                       "NVIDIA_DRIVER_CAPABILITIES")}
    env["CUDA_VISIBLE_DEVICES"] = "0"
    rec: dict = {"name": name, "started_unix_s": time.time(), "server_cmd": [str(PY), "main.py", "--listen", "127.0.0.1", "--port", "8188"]}
    proc = drv = None
    try:
        t0 = time.time()
        proc = subprocess.Popen(rec["server_cmd"], cwd=COMFY, env=env, stdout=open(res / "server.log", "ab"),
                                stderr=subprocess.STDOUT, start_new_session=True)
        drv = subprocess.Popen([str(PY), str(B / "comfy-driver.py"), str(res / "jobs.json"), str(res), str(COMFY), str(proc.pid), str(t0)],
                               cwd=B, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        done = 0
        for line in drv.stdout:
            try:
                ev = json.loads(line)
            except ValueError:
                tel.log(line.strip()[:800])
                continue
            if ev.get("ev") == "progress" and ev.get("max"):
                tel.progress(ev["value"] / ev["max"], stage=ev["label"], overall_fraction=min(0.99, (done + ev["value"] / ev["max"]) / len(jobs)))
            else:
                done += ev.get("ev") == "job_done"
                if ev.get("ev") == "server_ready":
                    rec["server_start_s"] = ev["s"]
                    rec["devices"] = ev.get("devices")
                tel.log(line.strip()[:800])
            if ctx.cancelled:
                drv.kill()
                break
        drv.wait(timeout=60)
        rec["driver_rc"] = drv.returncode
    except Exception as e:  # noqa: BLE001
        rec["error"] = repr(e)
    finally:
        if proc is not None and proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            for _ in range(300):
                if proc.poll() is not None:
                    break
                time.sleep(0.1)
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
    time.sleep(2)
    rows = nvml_rows()
    per, images = {}, []
    for job in jobs:
        p = res / f"{job['label']}.json"
        if not p.exists():
            per[job["label"]] = {"status": "not_run"}
            continue
        r = json.loads(p.read_text())
        a = r.get("submit_unix_s", 0)
        per[job["label"]] = r | window(rows, a, a + (r.get("total_s") or 0))
        images += [out.save_file(res / f, media_type="image/png") for f in r.get("files", []) if (res / f).exists()]
    rec["per_request"] = per
    rec["server_log_tail"] = (res / "server.log").read_text()[-2500:] if (res / "server.log").exists() else ""
    rec["seconds"] = round(time.time() - rec["started_unix_s"], 1)
    tel.log(json.dumps({k: {x: v.get(x) for x in ("status", "total_s", "h2d_gb", "peak_used_mib")} for k, v in per.items()}))
    return CellResult(json.dumps(rec), images)
