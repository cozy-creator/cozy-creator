"""Two full SDXL calls in one fresh process. Stock imports no Varena."""

import json
import os
import statistics
import sys
import threading
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Literal, TypedDict, cast

if TYPE_CHECKING:
    import tensorfs.varena.torch as vt
    from diffusers.pipelines.stable_diffusion_xl.pipeline_stable_diffusion_xl import (
        StableDiffusionXLPipeline,
    )

type JSON = None | bool | int | float | str | Sequence[JSON] | Mapping[str, JSON]


class Request(TypedDict):  # __init__.py's SDXL request, and where to put the images
    mode: str
    prompts: list[str]
    seed: int
    limit_bytes: int
    steps: int
    size: int
    out: str


class Report(TypedDict):
    mode: str
    torch: str
    gpu: str
    load_s: float
    adopt_s: float
    nvml_baseline: int
    calls: list[dict[str, JSON]]


request: Request = json.loads(Path(sys.argv[1]).read_text())
mode = request["mode"]
if mode.startswith("stock"):
    # Torch's own answer to the fragmentation that otherwise fails SDXL's VAE decode on 12 GB.
    os.environ["PYTORCH_CUDA_ALLOC_CONF"] = "expandable_segments:True"
else:
    import tensorfs.varena.torch as vt

    vt.install(0)
    if request["limit_bytes"]:
        vt.set_limit(0, request["limit_bytes"])

import diffusers
import huggingface_hub
import pynvml
import torch

huggingface_hub.utils.disable_progress_bars()  # type: ignore[attr-defined]  # huggingface_hub.utils re-exports it implicitly
model = huggingface_hub.snapshot_download(
    "stabilityai/stable-diffusion-xl-base-1.0", revision="462165984030d82259a11f4367a4eed129e94a7b",
    allow_patterns=["model_index.json", "scheduler/*", "tokenizer*/*", "*/config.json", "*.fp16.safetensors"],
    local_dir="/var/tmp/ci-torch-offload/sdxl-fp16",
)
pynvml.nvmlInit()
gpu = pynvml.nvmlDeviceGetHandleByIndex(0)


def used() -> int:
    return pynvml.nvmlDeviceGetMemoryInfo(gpu).used


baseline = used()
peak = [baseline]
stop = threading.Event()


def sample() -> None:
    while not stop.wait(0.02):
        peak[0] = max(peak[0], used())


threading.Thread(target=sample, daemon=True).start()


def host() -> dict[str, int]:
    rows = dict(line.split(":", 1) for line in Path("/proc/self/status").read_text().splitlines())
    return {key: int(rows[key].split()[0]) * 1024 for key in ("VmRSS", "RssAnon", "RssFile")}


started = time.perf_counter()
# For mypy, diffusers' __init__ binds its dummy stand-in first; at run time this is the real pipeline.
Pipeline = cast("type[StableDiffusionXLPipeline]", diffusers.StableDiffusionXLPipeline)
pipe = Pipeline.from_pretrained(model, torch_dtype=torch.float16, variant="fp16", use_safetensors=True, local_files_only=True,
                                add_watermarker=False)
pipe.set_progress_bar_config(disable=True)  # type: ignore[no-untyped-call]  # Diffusers leaves it unannotated
loaded = time.perf_counter()
handle = None
if mode == "stock":
    pipe.to("cuda")
elif mode == "stock-cpu-offload":
    pipe.enable_model_cpu_offload()
else:
    graphs: dict[str, Literal["auto", "eager"]] = {"eager": "eager", "graphs": "auto"}
    handle = vt.offload(pipe, device=0, graphs=graphs[mode])
report: Report = {"mode": mode, "torch": torch.__version__, "gpu": torch.cuda.get_device_name(0), "load_s": loaded - started,
          "adopt_s": time.perf_counter() - loaded, "nvml_baseline": baseline, "calls": []}


def counters() -> dict[str, JSON]:
    if handle is None:
        return {"allocated_peak": torch.cuda.max_memory_allocated(0), "mapped_peak": torch.cuda.max_memory_reserved(0)}
    stats = vt.stats(0)
    row: dict[str, JSON] = {"allocated_peak": stats["allocated_peak"], "mapped_peak": stats["mapped_peak"], "pools": stats["pools_bytes"], "weights_resident": stats["weights_resident"],
           "weights_filled": stats["weights_filled"], "event_waits": stats["event_waits"]}
    return {**row, "graphs": {**handle.graphs.counts}} if handle.graphs else row  # type: ignore[dict-item]  # GraphCounts: JSON values


for index, prompt in enumerate(request["prompts"]):
    if mode.startswith("stock"):
        torch.cuda.reset_peak_memory_stats(0)
    else:
        vt.reset_peak(0)
    peak[0] = used()
    events: "list[torch.cuda.Event]" = []

    def step_end(pipeline: "StableDiffusionXLPipeline", step: int, timestep: torch.Tensor,
                 kwargs: dict[str, torch.Tensor]) -> dict[str, torch.Tensor]:
        event = torch.cuda.Event(enable_timing=True)
        event.record(torch.cuda.current_stream(0))
        events.append(event)
        return kwargs

    begin, cpu = time.perf_counter(), time.process_time()
    first = torch.cuda.Event(enable_timing=True)
    first.record(torch.cuda.current_stream(0))
    failure = None
    try:
        with torch.inference_mode():
            image = pipe(prompt=prompt, negative_prompt="", height=request["size"], width=request["size"],
                         num_inference_steps=request["steps"], guidance_scale=5.0,
                         generator=torch.Generator("cpu").manual_seed(request["seed"] + index), callback_on_step_end=step_end).images[0]
        image.save(Path(request["out"]) / f"{mode}-{index}.png")
    except torch.OutOfMemoryError as error:
        failure = str(error)[:400]
    elapsed, cpu = time.perf_counter() - begin, time.process_time() - cpu
    events[-1].synchronize()
    steps = [events[i].elapsed_time(events[i + 1]) / 1000 for i in range(len(events) - 1)]
    row: dict[str, JSON] = {"call": index + 1, "prompt": prompt, "total_s": elapsed, "cpu_s": cpu, "failure": failure, "to_first_step_s": first.elapsed_time(events[0]) / 1000,
           "steady_step_s": statistics.median(steps), "steps_s": [round(s, 4) for s in steps], "nvml_peak": peak[0], "host": host(), **counters()}
    report["calls"].append(row)
    print(json.dumps({"call": row}), flush=True)
    if failure:  # Stock Torch's own limit: its VAE stays half upcast, so a next call only says that.
        break
stop.set()
print(json.dumps({"report": report}), flush=True)
