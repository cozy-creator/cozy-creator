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
    files: list[FileAsset]


CELL = 'qwen21'
JOBS = json.loads('[{"label": "qwen21-comfy-cold-00", "cell": "qwen21", "engine": "comfy", "mode": "cold", "sample": 0, "seed": 202610261000, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-same-prompt-warm-01", "cell": "qwen21", "engine": "comfy", "mode": "same-prompt-warm", "sample": 1, "seed": 202610261001, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-same-prompt-warm-02", "cell": "qwen21", "engine": "comfy", "mode": "same-prompt-warm", "sample": 2, "seed": 202610261002, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-same-prompt-warm-03", "cell": "qwen21", "engine": "comfy", "mode": "same-prompt-warm", "sample": 3, "seed": 202610261003, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-same-prompt-warm-04", "cell": "qwen21", "engine": "comfy", "mode": "same-prompt-warm", "sample": 4, "seed": 202610261004, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-same-prompt-warm-05", "cell": "qwen21", "engine": "comfy", "mode": "same-prompt-warm", "sample": 5, "seed": 202610261005, "prompt": "A photograph of an astronaut riding a horse, natural daylight, detailed landscape.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-changed-prompt-warm-06", "cell": "qwen21", "engine": "comfy", "mode": "changed-prompt-warm", "sample": 6, "seed": 202610261006, "prompt": "A photograph of a small red sailboat on a calm blue lake beneath distant mountains.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-changed-prompt-warm-07", "cell": "qwen21", "engine": "comfy", "mode": "changed-prompt-warm", "sample": 7, "seed": 202610261007, "prompt": "A photograph of a golden retriever sitting beside a bicycle in a quiet city park.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-changed-prompt-warm-08", "cell": "qwen21", "engine": "comfy", "mode": "changed-prompt-warm", "sample": 8, "seed": 202610261008, "prompt": "A photograph of a stone lighthouse on a rocky coast under an overcast sky.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-changed-prompt-warm-09", "cell": "qwen21", "engine": "comfy", "mode": "changed-prompt-warm", "sample": 9, "seed": 202610261009, "prompt": "A photograph of a wooden cabin surrounded by pine trees after fresh snowfall.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}, {"label": "qwen21-comfy-changed-prompt-warm-10", "cell": "qwen21", "engine": "comfy", "mode": "changed-prompt-warm", "sample": 10, "seed": 202610261010, "prompt": "A photograph of a ceramic teapot and two cups on a sunlit kitchen table.", "negative_prompt": "", "size": 1024, "width": 1024, "height": 1024, "steps": 30, "guidance": 1.0, "batch": 1, "hidiffusion": false, "prompt_upsampling": false, "lora": null, "reference_images": [], "expected_gpu_name": "RTX 4090"}]')
IMAGES = {}

B = Path("/var/tmp/bench4090-20261026")
COMFY = B / "ComfyUI"
PY = B / "comfy-env/bin/python"
KEEP_ENV = ("PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR", "USER", "CUDA_VISIBLE_DEVICES", "NVIDIA_VISIBLE_DEVICES",
            "NVIDIA_DRIVER_CAPABILITIES", "LD_LIBRARY_PATH")
DRIVER = '"""Runs in ComfyUI\'s venv beside its server: one cell\'s jobs over the HTTP and websocket API, one at a time.\n\n    driver.py <session> <jobs.json> <out-dir> <server-pid> <server-start-unix-s>\n\nPrints one JSON line per event for the cozy script. Graphs use ComfyUI\'s own nodes wired as its templates wire\nthem, with Cozy\'s steps, CFG and size for the cell.\n"""\n\nimport asyncio\nimport math\n\nimport hashlib\n\nimport json\n\nimport os\n\nimport shutil\n\nimport sys\n\nimport time\n\nimport urllib.request\n\nimport uuid\n\nfrom pathlib import Path\n\nimport aiohttp\n\nURL = "http://127.0.0.1:8188"\n\nCOMFY = Path("/var/tmp/bench4090-20261026/ComfyUI")\n\nSDXL = "sd_xl_base_1.0.safetensors"\n\ndef node(kind: str, **inputs: object) -> dict:\n    return {"class_type": kind, "inputs": inputs}\n\n\ndef sdxl(job: dict, match: dict) -> dict:\n    assert match["weights_match"] is True\n    assert job["size"] == 1024 and job["steps"] == 30 and job.get("batch", 1) == 1\n    assert float(job["guidance"]) == 7.0\n    assert not job.get("lora")\n    graph = {\n        "1": node("CheckpointLoaderSimple", ckpt_name="sd_xl_base_1.0.safetensors"),\n        "2": node("CLIPTextEncode", clip=["1", 1], text=job["prompt"]),\n        "3": node("CLIPTextEncode", clip=["1", 1], text=job.get("negative_prompt", "")),\n        "4": node("EmptyLatentImage", width=1024, height=1024, batch_size=1),\n        "6": node("VAEDecode", samples=["5", 0], vae=["1", 2]),\n        "7": node("SaveImage", images=["6", 0], filename_prefix=f"bench/{job[\'label\']}"),\n    }\n    common = {"model": ["1", 0], "cfg": 7.0, "positive": ["2", 0],\n              "negative": ["3", 0], "latent_image": ["4", 0]}\n    if match["comfy_scheduler"] == "normal":\n        graph["5"] = node("KSampler", **common, seed=job["seed"], steps=30,\n                          sampler_name="euler", scheduler="normal", denoise=1.0)\n    else:\n        assert match["comfy_scheduler"] == "manual-cozy-observed"\n        sigmas = match["cozy_sigmas"]\n        assert len(sigmas) == 31 and all(math.isfinite(x) and x >= 0 for x in sigmas)\n        assert sigmas[-1] == 0 and all(a > b for a, b in zip(sigmas, sigmas[1:]))\n        # ManualSigmas uses a decimal-only regex in fb3; scientific notation is unsafe.\n        graph["8"] = node("ManualSigmas", sigmas=", ".join(format(x, ".17f") for x in sigmas))\n        graph["9"] = node("KSamplerSelect", sampler_name="euler")\n        # Match the scheduler\'s initial noise scale using existing nodes. AddNoise\n        # returns an external latent representation; SamplerCustom converts it back\n        # through process_latent_in before sampling with additional noise disabled.\n        initial = match["cozy_init_noise_sigma"]\n        assert isinstance(initial, (int, float)) and math.isfinite(initial) and initial > 0\n        graph["10"] = node("RandomNoise", noise_seed=job["seed"])\n        graph["11"] = node("ManualSigmas", sigmas=format(initial, ".17f"))\n        graph["12"] = node("AddNoise", model=["1", 0], noise=["10", 0],\n                           sigmas=["11", 0], latent_image=["4", 0])\n        common["latent_image"] = ["12", 0]\n        graph["5"] = node("SamplerCustom", **common, add_noise=False, noise_seed=job["seed"],\n                          sampler=["9", 0], sigmas=["8", 0])\n    return graph\n\ndef qwen21(job,match):\n    assert match[\'matched\'] is True\n    assert job[\'size\']==1024 and job[\'steps\']==30 and job.get(\'batch\',1)==1\n    assert not job.get(\'reference_images\') and not job.get(\'lora\')\n    assert float(job[\'guidance\'])==1\n    files=match[\'comfy_files\']\n    graph={\n        \'1\':node(\'UNETLoader\',unet_name=Path(files[\'transformer\'][\'path\']).name,weight_dtype=\'default\'),\n        \'2\':node(\'QwenImage21Cache\',model=[\'1\',0],device=\'auto\',dtype=\'default\'),\n        \'3\':node(\'CLIPLoader\',clip_name=Path(files[\'text_encoder\'][\'path\']).name,type=\'qwen_image\',device=\'default\'),\n        \'4\':node(\'VAELoader\',vae_name=Path(files[\'vae\'][\'path\']).name),\n        \'5\':node(\'TextEncodeQwenImage21\',clip=[\'3\',0],prompt=job[\'prompt\'],negative_prompt=\'\',resolution=1024),\n        \'8\':node(\'VAEDecode\',samples=[\'7\',0],vae=[\'4\',0]),\n        \'9\':node(\'SaveImage\',images=[\'8\',0],filename_prefix=f"bench/{job[\'label\']}"),\n    }\n    common={\'model\':[\'2\',0],\'cfg\':1.0,\'positive\':[\'5\',0],\'negative\':[\'5\',1],\'latent_image\':[\'5\',2]}\n    if match[\'comfy_scheduler\']==\'simple\':\n        graph[\'7\']=node(\'KSampler\',**common,seed=job[\'seed\'],steps=30,sampler_name=\'euler\',scheduler=\'simple\',denoise=1.0)\n    else:\n        assert match[\'comfy_scheduler\']==\'manual-cozy-observed\'\n        sigmas=match[\'cozy_sigmas\'];assert len(sigmas)==31 and sigmas[0]==1 and sigmas[-1]==0 and all(math.isfinite(v) for v in sigmas)\n        graph[\'10\']=node(\'ManualSigmas\',sigmas=\', \'.join(format(x,\'.17f\') for x in sigmas))\n        graph[\'11\']=node(\'KSamplerSelect\',sampler_name=\'euler\')\n        graph[\'7\']=node(\'SamplerCustom\',**common,add_noise=True,noise_seed=job[\'seed\'],sampler=[\'11\',0],sigmas=[\'10\',0])\n    return graph\n\n\ndef emit(**kw):\n    print(json.dumps(kw), flush=True)\n\nasync def run_one(session, g, job, out):\n    client, events, label = uuid.uuid4().hex, [], job["label"]\n    if job.get("profile"):\n        hook = json.loads((COMFY.parent / "profiles/module-ready.json").read_text())\n        assert hook["pid"] == int(sys.argv[4]), "diagnostic hook did not load in this server"\n    async with session.ws_connect(f"ws://127.0.0.1:8188/ws?clientId={client}", max_msg_size=0, heartbeat=30) as ws:\n        t0 = time.time()\n        mono = time.monotonic()\n        async with session.post(URL + "/prompt", json={"prompt": g, "client_id": client, "extra_data": {"benchmark_profile": job.get("profile", False)}}) as r:\n            body = await r.json()\n            if r.status != 200:\n                return {"label": label, "status": "rejected", "body": body, "submit_unix_s": t0, "total_s": 0, "events": []}\n        pid = body["prompt_id"]\n        while True:\n            msg = await ws.receive()\n            now = time.monotonic() - mono\n            if msg.type == aiohttp.WSMsgType.BINARY:\n                continue\n            if msg.type != aiohttp.WSMsgType.TEXT:\n                events.append({"t": round(now, 4), "type": "ws_closed", "data": {}})\n                break\n            m = json.loads(msg.data)\n            data = m.get("data") or {}\n            if m["type"] in ("status", "progress_state", "crystools.monitor") or data.get("prompt_id") not in (None, pid):\n                continue\n            events.append({"t": round(now, 4), "type": m["type"], "data": data})\n            if m["type"] == "progress":\n                emit(ev="progress", label=label, value=data.get("value"), max=data.get("max"), t=round(now, 2))\n            elif m["type"] == "executing":\n                emit(ev="executing", label=label, node=data.get("node"), cls=(g.get(data.get("node") or "") or {}).get("class_type"), t=round(now, 2))\n            else:\n                emit(ev=m["type"], label=label, t=round(now, 2))\n            if m["type"] in ("execution_success", "execution_error", "execution_interrupted"):\n                break\n    total = events[-1]["t"] if events else 0\n    profile_receipt = None\n    if job.get("profile"):\n        ready = COMFY.parent / "profiles" / f"{pid}.ready.json"\n        # execution_success precedes profiler serialization and history publication.\n        # Poll the explicit diagnostic completion marker, under the outer rental watchdog.\n        while not ready.exists():\n            failed = ready.with_name(f"{pid}.failed.json")\n            assert not failed.exists(), failed.read_text() if failed.exists() else ""\n            server_pid = int(sys.argv[4])\n            assert Path(f"/proc/{server_pid}").exists(), "server ended before profile export"\n            await asyncio.sleep(0.25)\n        profile_receipt = json.loads(ready.read_text())\n        assert profile_receipt["prompt_id"] == pid and profile_receipt["success"]\n        source = Path(profile_receipt["path"])\n        assert hashlib.sha256(source.read_bytes()).hexdigest() == profile_receipt["sha256"]\n        target = out / f"{label}.trace.json"\n        shutil.copy2(source, target)\n        profile_receipt["copied_file"] = target.name\n    while True:\n        async with session.get(f"{URL}/history/{pid}") as response:\n            history = (await response.json()).get(pid)\n        if history:\n            break\n        assert Path(f"/proc/{int(sys.argv[4])}").exists(), "server ended before history publication"\n        await asyncio.sleep(0.05)\n    files, texts = [], []\n    for node_out in history.get("outputs", {}).values():\n        texts += [t for t in node_out.get("text", []) if isinstance(t, str)]\n        for items in node_out.values():  # images, videos, audio: every saved file\n            for item in items if isinstance(items, list) else []:\n                if isinstance(item, dict) and "filename" in item:\n                    src = COMFY / item.get("type", "output") / item.get("subfolder", "") / item["filename"]\n                    dst = out / f"{label}-{len(files)}{src.suffix}"\n                    if src.exists():\n                        shutil.copy2(src, dst)\n                        files.append(dst.name)\n    delivery_s = time.monotonic() - mono\n    observed = [e["data"] for e in events if e["type"] == "progress" and str(e["data"].get("node")) == ("5" if job["cell"] == "sdxl" else "7")]\n    values = list(dict.fromkeys(e.get("value") for e in observed))\n    assert values == list(range(1, 31)), ("incomplete actual SDXL steps", values)\n    assert all(e.get("max") == 30 for e in observed), observed\n    assert events[-1]["type"] == "execution_success", events[-1]\n    assert ("5" if job["cell"] == "sdxl" else "7") not in [str(n) for e in events if e["type"] == "execution_cached" for n in e["data"].get("nodes", [])]\n    from PIL import Image\n    images = []\n    for filename in files:\n        path = out / filename\n        with Image.open(path) as picture:\n            picture.load()\n            assert picture.size == (1024, 1024), picture.size\n            rgb = picture.convert("RGB")\n            assert any(lo != hi for lo, hi in rgb.getextrema()), "flat output"\n            images.append({"file": filename, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),\n                           "rgb_sha256": hashlib.sha256(rgb.tobytes()).hexdigest(), "format": picture.format})\n    assert len(images) == 1, images\n    segments, cur = [], None\n    for e in events:\n        if e["type"] in ("executing", "execution_success", "execution_error", "execution_interrupted", "ws_closed"):\n            if cur:\n                cur["end"] = e["t"]\n                cur["s"] = round(cur["end"] - cur["start"], 3)\n                segments.append(cur)\n            n = e["data"].get("node") if e["type"] == "executing" else None\n            cur = {"node": n, "class": g.get(n, {}).get("class_type"), "start": e["t"]} if n else None\n    progress = [(e["t"], e["data"].get("value")) for e in events if e["type"] == "progress" and str(e["data"].get("node")) == ("5" if job["cell"] == "sdxl" else "7")]\n    steps = [round(b[0] - a[0], 4) for a, b in zip(progress, progress[1:])]\n    res = {"label": label, "prompt_id": pid, "status": events[-1]["type"] if events else "none", "total_s": total,\n           "submit_unix_s": t0, "profile": profile_receipt, "request_through_local_copy_s": delivery_s, "observed_steps": len(values), "images": images, "segments": segments, "steps": steps, "files": files,\n           "cached_nodes": [n for e in events if e["type"] == "execution_cached" for n in e["data"].get("nodes", [])],\n           "error": [e["data"] for e in events if e["type"] == "execution_error"], "events": events}\n    return res\n\nasync def main():\n    jobs, out = json.loads(Path(sys.argv[2]).read_text()), Path(sys.argv[3])\n    spid, t0 = int(sys.argv[4]), float(sys.argv[5])\n    alive = lambda: os.path.exists(f"/proc/{spid}") and open(f"/proc/{spid}/stat").read().split()[2] != "Z"\n    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=None)) as session:\n        while True:\n            if not alive():\n                emit(ev="server_exited_during_start")\n                return\n            try:\n                async with session.get(URL + "/system_stats") as r:\n                    stats = await r.json()\n                break\n            except aiohttp.ClientError:\n                await asyncio.sleep(0.25)\n        devices = stats.get("devices", [])\n        assert len(devices) == 1 and devices[0].get("type") == "cuda", stats\n        expected_names = {job["expected_gpu_name"] for job in jobs}\n        assert len(expected_names) == 1, expected_names\n        expected_name = next(iter(expected_names))\n        assert isinstance(expected_name, str) and expected_name.strip(), expected_name\n        assert expected_name in devices[0].get("name", ""), (expected_name, stats)\n        (out / "server.json").write_text(json.dumps({"server_start_s": round(time.time() - t0, 2), "system_stats": stats}, indent=1))\n        emit(ev="server_ready", s=round(time.time() - t0, 2))\n        for job in jobs:\n            if not alive():\n                emit(ev="server_dead_before", label=job["label"])\n                break\n            assert job["cell"] in ("sdxl", "qwen21")\n            match = json.loads((COMFY.parent / ("matched-sdxl.json" if job["cell"] == "sdxl" else "matched-qwen.json")).read_text())\n            g = sdxl(job, match) if job["cell"] == "sdxl" else qwen21(job, match)\n            (out / f"{job[\'label\']}.graph.json").write_text(json.dumps(g, indent=1, ensure_ascii=False))\n            emit(ev="job_start", label=job["label"])\n            try:\n                res = await run_one(session, g, job, out)\n            except Exception as e:  # noqa: BLE001\n                res = {"label": job["label"], "status": "driver_error", "error": repr(e), "total_s": 0, "submit_unix_s": time.time()}\n            res["job"] = job\n            (out / f"{job[\'label\']}.json").write_text(json.dumps(res, indent=1, ensure_ascii=False))\n            if res["status"] != "execution_success":\n                raise RuntimeError(f"stop after failed Comfy job {job[\'label\']}: {res}")\n            emit(ev="job_done", label=job["label"], status=res["status"], total_s=res.get("total_s"), tokens=res.get("tokens"),\n                 segments=[(s["class"], s["s"]) for s in res.get("segments", [])], error=str(res.get("error"))[:600])\n\nif __name__ == "__main__":\n    asyncio.run(main())\n'


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
        return BenchResult(files=[out.save_file(path, media_type="application/json"),
                                  out.save_file(archive, media_type="application/gzip")])
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
        assert drv.returncode == 0 and result["status"] == "execution_success", result
        assert result["observed_steps"] == job["steps"]
        rec["request"] = result
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
    saved = [out.save_file(res / "session.json", media_type="application/json")]
    if healthy:
        for image in rec["request"]["images"]:
            saved.append(out.save_file(res / image["file"], media_type="image/png"))
        if rec["request"].get("profile"):
            saved.append(out.save_file(res / rec["request"]["profile"]["copied_file"], media_type="application/json"))
    # Detailed logs/timelines are delivered by the explicit untimed stop operation.
    # Timed calls return the image directly plus only their compact session receipt.
    return BenchResult(files=saved)
