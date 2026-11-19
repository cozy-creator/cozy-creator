"""se-172 proof: ComfyUI's side of the LTX profile. On PYTHONPATH of the benchmarks runner's ComfyUI server, it records sampling
steps 2-4 of stage 1 and 2-3 of stage 2 with torch.profiler for a request whose prompt carries [profile], into /var/tmp/ltx-prof
(key averages and a Chrome trace). Other requests and other programs pass through untouched."""
import importlib.abc
import importlib.util
import sys

STATE = {"label": ""}
DIR = "/var/tmp/ltx-prof"


def _write(prof, label):
    try:
        import gzip
        import json
        import os
        os.makedirs(DIR, exist_ok=True)
        rows = []
        for e in prof.key_averages():
            dev = getattr(e, "device_time_total", None)
            if dev is None:
                dev = getattr(e, "cuda_time_total", 0)
            rows.append({"name": e.key, "count": e.count, "device_us": dev,
                         "self_device_us": getattr(e, "self_device_time_total", getattr(e, "self_cuda_time_total", 0)),
                         "cpu_us": e.cpu_time_total, "self_cpu_us": e.self_cpu_time_total})
        with open(os.path.join(DIR, f"comfy-{label}.json"), "w") as f:
            json.dump(rows, f)
        with open(os.path.join(DIR, f"comfy-{label}.txt"), "w") as f:
            f.write(prof.key_averages().table(sort_by="self_device_time_total", row_limit=80))
        path = os.path.join(DIR, f"comfy-{label}.trace.json")
        prof.export_chrome_trace(path)
        with open(path, "rb") as src, gzip.open(path + ".gz", "wb") as dst:
            dst.write(src.read())
        os.remove(path)
    except Exception as e:  # a profile that can't be written never fails the request
        print(f"ltxprof {label}: not written: {e!r}", file=sys.stderr)


def _patch_nodes(module):
    cls = getattr(module, "CLIPTextEncode", None)
    if cls is None or not hasattr(cls, "encode"):
        return
    orig = cls.encode

    def encode(self, clip, text, *a, **k):
        if isinstance(text, str) and "[profile]" in text:
            import time
            STATE["label"] = f"prof-{int(time.time())}"
        return orig(self, clip, text, *a, **k)
    cls.encode = encode


def _patch_sampling(module):
    orig = module.sample_euler

    def sample_euler(model, x, sigmas, *a, callback=None, **k):
        label = STATE["label"]
        if not label:
            return orig(model, x, sigmas, *a, callback=callback, **k)
        import torch
        stage = 1 if len(sigmas) - 1 >= 8 else 2
        prof = []

        def cb(d):
            if callback is not None:
                callback(d)
            if d.get("i") == 0 and not prof:
                torch.cuda.synchronize()
                prof.append(torch.profiler.profile(activities=[torch.profiler.ProfilerActivity.CPU, torch.profiler.ProfilerActivity.CUDA]))
                prof[0].start()
            elif prof and d.get("i") == 3:
                torch.cuda.synchronize()
                prof[0].stop()
                _write(prof.pop(), f"{label}-s{stage}")
        try:
            return orig(model, x, sigmas, *a, callback=cb, **k)
        finally:
            if prof:
                torch.cuda.synchronize()
                prof[0].stop()
                _write(prof.pop(), f"{label}-s{stage}")
            if stage == 2:
                STATE["label"] = ""
    module.sample_euler = sample_euler


PATCHES = {"nodes": _patch_nodes, "comfy.k_diffusion.sampling": _patch_sampling}


class _Finder(importlib.abc.MetaPathFinder):
    def find_spec(self, name, path, target=None):
        if name not in PATCHES:
            return None
        sys.meta_path.remove(self)
        try:
            spec = importlib.util.find_spec(name)
        finally:
            sys.meta_path.insert(0, self)
        if spec is None or spec.loader is None:
            return spec
        loader, patch = spec.loader, PATCHES[name]
        run = loader.exec_module

        def exec_module(module):
            run(module)
            try:
                patch(module)
            except Exception as e:
                print(f"ltxprof: {name} not patched: {e!r}", file=sys.stderr)
        loader.exec_module = exec_module
        return spec


sys.meta_path.insert(0, _Finder())
