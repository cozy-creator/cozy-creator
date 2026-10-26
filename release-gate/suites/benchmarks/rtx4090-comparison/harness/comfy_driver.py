"""Runs in ComfyUI's venv beside its server: one cell's jobs over the HTTP and websocket API, one at a time.

    driver.py <session> <jobs.json> <out-dir> <server-pid> <server-start-unix-s>

Prints one JSON line per event for the cozy script. Graphs use ComfyUI's own nodes wired as its templates wire
them, with Cozy's steps, CFG and size for the cell.
"""

import asyncio
import math

import hashlib

import json

import os

import shutil

import sys

import time

import urllib.request

import uuid

from pathlib import Path

import aiohttp

URL = "http://127.0.0.1:8188"

COMFY = Path("/var/tmp/bench4090-20261026/ComfyUI")

SDXL = "sd_xl_base_1.0.safetensors"

def node(kind: str, **inputs: object) -> dict:
    return {"class_type": kind, "inputs": inputs}


def sdxl(job: dict, match: dict) -> dict:
    assert match["weights_match"] is True
    assert job["size"] == 1024 and job["steps"] == 30 and job.get("batch", 1) == 1
    assert float(job["guidance"]) == 7.0
    assert not job.get("lora")
    graph = {
        "1": node("CheckpointLoaderSimple", ckpt_name="sd_xl_base_1.0.safetensors"),
        "2": node("CLIPTextEncode", clip=["1", 1], text=job["prompt"]),
        "3": node("CLIPTextEncode", clip=["1", 1], text=job.get("negative_prompt", "")),
        "4": node("EmptyLatentImage", width=1024, height=1024, batch_size=1),
        "6": node("VAEDecode", samples=["5", 0], vae=["1", 2]),
        "7": node("SaveImage", images=["6", 0], filename_prefix=f"bench/{job['label']}"),
    }
    common = {"model": ["1", 0], "cfg": 7.0, "positive": ["2", 0],
              "negative": ["3", 0], "latent_image": ["4", 0]}
    if match["comfy_scheduler"] == "normal":
        graph["5"] = node("KSampler", **common, seed=job["seed"], steps=30,
                          sampler_name="euler", scheduler="normal", denoise=1.0)
    else:
        assert match["comfy_scheduler"] == "manual-cozy-observed"
        sigmas = match["cozy_sigmas"]
        assert len(sigmas) == 31 and all(math.isfinite(x) and x >= 0 for x in sigmas)
        assert sigmas[-1] == 0 and all(a > b for a, b in zip(sigmas, sigmas[1:]))
        # ManualSigmas uses a decimal-only regex in fb3; scientific notation is unsafe.
        graph["8"] = node("ManualSigmas", sigmas=", ".join(format(x, ".17f") for x in sigmas))
        graph["9"] = node("KSamplerSelect", sampler_name="euler")
        # Match the scheduler's initial noise scale using existing nodes. AddNoise
        # returns an external latent representation; SamplerCustom converts it back
        # through process_latent_in before sampling with additional noise disabled.
        initial = match["cozy_init_noise_sigma"]
        assert isinstance(initial, (int, float)) and math.isfinite(initial) and initial > 0
        graph["10"] = node("RandomNoise", noise_seed=job["seed"])
        graph["11"] = node("ManualSigmas", sigmas=format(initial, ".17f"))
        graph["12"] = node("AddNoise", model=["1", 0], noise=["10", 0],
                           sigmas=["11", 0], latent_image=["4", 0])
        common["latent_image"] = ["12", 0]
        graph["5"] = node("SamplerCustom", **common, add_noise=False, noise_seed=job["seed"],
                          sampler=["9", 0], sigmas=["8", 0])
    return graph

def qwen21(job,match):
    assert match['matched'] is True
    assert job['size']==1024 and job['steps']==30 and job.get('batch',1)==1
    assert not job.get('reference_images') and not job.get('lora')
    assert float(job['guidance'])==1
    files=match['comfy_files']
    graph={
        '1':node('UNETLoader',unet_name=Path(files['transformer']['path']).name,weight_dtype='default'),
        '2':node('QwenImage21Cache',model=['1',0],device='auto',dtype='default'),
        '3':node('CLIPLoader',clip_name=Path(files['text_encoder']['path']).name,type='qwen_image',device='default'),
        '4':node('VAELoader',vae_name=Path(files['vae']['path']).name),
        '5':node('TextEncodeQwenImage21',clip=['3',0],prompt=job['prompt'],negative_prompt='',resolution=1024),
        '8':node('VAEDecode',samples=['7',0],vae=['4',0]),
        '9':node('SaveImage',images=['8',0],filename_prefix=f"bench/{job['label']}"),
    }
    common={'model':['2',0],'cfg':1.0,'positive':['5',0],'negative':['5',1],'latent_image':['5',2]}
    if match['comfy_scheduler']=='simple':
        graph['7']=node('KSampler',**common,seed=job['seed'],steps=30,sampler_name='euler',scheduler='simple',denoise=1.0)
    else:
        assert match['comfy_scheduler']=='manual-cozy-observed'
        sigmas=match['cozy_sigmas'];assert len(sigmas)==31 and sigmas[0]==1 and sigmas[-1]==0 and all(math.isfinite(v) for v in sigmas)
        graph['10']=node('ManualSigmas',sigmas=', '.join(format(x,'.17f') for x in sigmas))
        graph['11']=node('KSamplerSelect',sampler_name='euler')
        graph['7']=node('SamplerCustom',**common,add_noise=True,noise_seed=job['seed'],sampler=['11',0],sigmas=['10',0])
    return graph


def emit(**kw):
    print(json.dumps(kw), flush=True)

async def run_one(session, g, job, out):
    client, events, label = uuid.uuid4().hex, [], job["label"]
    if job.get("profile"):
        hook = json.loads((COMFY.parent / "profiles/module-ready.json").read_text())
        assert hook["pid"] == int(sys.argv[4]), "diagnostic hook did not load in this server"
    async with session.ws_connect(f"ws://127.0.0.1:8188/ws?clientId={client}", max_msg_size=0, heartbeat=30) as ws:
        t0 = time.time()
        mono = time.monotonic()
        async with session.post(URL + "/prompt", json={"prompt": g, "client_id": client, "extra_data": {"benchmark_profile": job.get("profile", False)}}) as r:
            body = await r.json()
            if r.status != 200:
                return {"label": label, "status": "rejected", "body": body, "submit_unix_s": t0, "total_s": 0, "events": []}
        pid = body["prompt_id"]
        while True:
            msg = await ws.receive()
            now = time.monotonic() - mono
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
            elif m["type"] == "executing":
                emit(ev="executing", label=label, node=data.get("node"), cls=(g.get(data.get("node") or "") or {}).get("class_type"), t=round(now, 2))
            else:
                emit(ev=m["type"], label=label, t=round(now, 2))
            if m["type"] in ("execution_success", "execution_error", "execution_interrupted"):
                break
    total = events[-1]["t"] if events else 0
    profile_receipt = None
    if job.get("profile"):
        ready = COMFY.parent / "profiles" / f"{pid}.ready.json"
        # execution_success precedes profiler serialization and history publication.
        # Poll the explicit diagnostic completion marker, under the outer rental watchdog.
        while not ready.exists():
            failed = ready.with_name(f"{pid}.failed.json")
            assert not failed.exists(), failed.read_text() if failed.exists() else ""
            server_pid = int(sys.argv[4])
            assert Path(f"/proc/{server_pid}").exists(), "server ended before profile export"
            await asyncio.sleep(0.25)
        profile_receipt = json.loads(ready.read_text())
        assert profile_receipt["prompt_id"] == pid and profile_receipt["success"]
        source = Path(profile_receipt["path"])
        assert hashlib.sha256(source.read_bytes()).hexdigest() == profile_receipt["sha256"]
        target = out / f"{label}.trace.json"
        shutil.copy2(source, target)
        profile_receipt["copied_file"] = target.name
    while True:
        async with session.get(f"{URL}/history/{pid}") as response:
            history = (await response.json()).get(pid)
        if history:
            break
        assert Path(f"/proc/{int(sys.argv[4])}").exists(), "server ended before history publication"
        await asyncio.sleep(0.05)
    files, texts = [], []
    for node_out in history.get("outputs", {}).values():
        texts += [t for t in node_out.get("text", []) if isinstance(t, str)]
        for items in node_out.values():  # images, videos, audio: every saved file
            for item in items if isinstance(items, list) else []:
                if isinstance(item, dict) and "filename" in item:
                    src = COMFY / item.get("type", "output") / item.get("subfolder", "") / item["filename"]
                    dst = out / f"{label}-{len(files)}{src.suffix}"
                    if src.exists():
                        shutil.copy2(src, dst)
                        files.append(dst.name)
    delivery_s = time.monotonic() - mono
    observed = [e["data"] for e in events if e["type"] == "progress" and str(e["data"].get("node")) == ("5" if job["cell"] == "sdxl" else "7")]
    values = list(dict.fromkeys(e.get("value") for e in observed))
    assert values == list(range(1, 31)), ("incomplete actual SDXL steps", values)
    assert all(e.get("max") == 30 for e in observed), observed
    assert events[-1]["type"] == "execution_success", events[-1]
    assert ("5" if job["cell"] == "sdxl" else "7") not in [str(n) for e in events if e["type"] == "execution_cached" for n in e["data"].get("nodes", [])]
    from PIL import Image
    images = []
    for filename in files:
        path = out / filename
        with Image.open(path) as picture:
            picture.load()
            assert picture.size == (1024, 1024), picture.size
            rgb = picture.convert("RGB")
            assert any(lo != hi for lo, hi in rgb.getextrema()), "flat output"
            images.append({"file": filename, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                           "rgb_sha256": hashlib.sha256(rgb.tobytes()).hexdigest(), "format": picture.format})
    assert len(images) == 1, images
    segments, cur = [], None
    for e in events:
        if e["type"] in ("executing", "execution_success", "execution_error", "execution_interrupted", "ws_closed"):
            if cur:
                cur["end"] = e["t"]
                cur["s"] = round(cur["end"] - cur["start"], 3)
                segments.append(cur)
            n = e["data"].get("node") if e["type"] == "executing" else None
            cur = {"node": n, "class": g.get(n, {}).get("class_type"), "start": e["t"]} if n else None
    progress = [(e["t"], e["data"].get("value")) for e in events if e["type"] == "progress" and str(e["data"].get("node")) == ("5" if job["cell"] == "sdxl" else "7")]
    steps = [round(b[0] - a[0], 4) for a, b in zip(progress, progress[1:])]
    res = {"label": label, "prompt_id": pid, "status": events[-1]["type"] if events else "none", "total_s": total,
           "submit_unix_s": t0, "profile": profile_receipt, "request_through_local_copy_s": delivery_s, "observed_steps": len(values), "images": images, "segments": segments, "steps": steps, "files": files,
           "cached_nodes": [n for e in events if e["type"] == "execution_cached" for n in e["data"].get("nodes", [])],
           "error": [e["data"] for e in events if e["type"] == "execution_error"], "events": events}
    return res

async def main():
    jobs, out = json.loads(Path(sys.argv[2]).read_text()), Path(sys.argv[3])
    spid, t0 = int(sys.argv[4]), float(sys.argv[5])
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
        devices = stats.get("devices", [])
        assert len(devices) == 1 and devices[0].get("type") == "cuda", stats
        expected_names = {job["expected_gpu_name"] for job in jobs}
        assert len(expected_names) == 1, expected_names
        expected_name = next(iter(expected_names))
        assert isinstance(expected_name, str) and expected_name.strip(), expected_name
        assert expected_name in devices[0].get("name", ""), (expected_name, stats)
        (out / "server.json").write_text(json.dumps({"server_start_s": round(time.time() - t0, 2), "system_stats": stats}, indent=1))
        emit(ev="server_ready", s=round(time.time() - t0, 2))
        for job in jobs:
            if not alive():
                emit(ev="server_dead_before", label=job["label"])
                break
            assert job["cell"] in ("sdxl", "qwen21")
            match = json.loads((COMFY.parent / ("matched-sdxl.json" if job["cell"] == "sdxl" else "matched-qwen.json")).read_text())
            g = sdxl(job, match) if job["cell"] == "sdxl" else qwen21(job, match)
            (out / f"{job['label']}.graph.json").write_text(json.dumps(g, indent=1, ensure_ascii=False))
            emit(ev="job_start", label=job["label"])
            try:
                res = await run_one(session, g, job, out)
            except Exception as e:  # noqa: BLE001
                res = {"label": job["label"], "status": "driver_error", "error": repr(e), "total_s": 0, "submit_unix_s": time.time()}
            res["job"] = job
            (out / f"{job['label']}.json").write_text(json.dumps(res, indent=1, ensure_ascii=False))
            if res["status"] != "execution_success":
                raise RuntimeError(f"stop after failed Comfy job {job['label']}: {res}")
            emit(ev="job_done", label=job["label"], status=res["status"], total_s=res.get("total_s"), tokens=res.get("tokens"),
                 segments=[(s["class"], s["s"]) for s in res.get("segments", [])], error=str(res.get("error"))[:600])

if __name__ == "__main__":
    asyncio.run(main())
