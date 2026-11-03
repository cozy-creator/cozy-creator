"""Actual 8 GiB hardware: unchanged adopted-only two-call SDXL; no managed cap or retry."""
from __future__ import annotations
import argparse
import csv
import hashlib
import json
import os
import runpy
import subprocess
import time
import traceback
from pathlib import Path
import numpy as np
import torch
from diffusers import StableDiffusionXLPipeline
from tensorfs import _ext
from tensorfs.varena import api
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("model")
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--variant", default="fp16")
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--steps", type=int, default=20)
parser.add_argument("--prompt", default="a red ceramic teapot on a wooden table")
parser.add_argument("--gpu-uuid", required=True)
parser.add_argument("--expected-sku", required=True, help="operator-bound provider SKU, verified against the hardware receipt")
parser.add_argument("--expected-device-name", required=True)
parser.add_argument("--expected-total-bytes", type=int, required=True)
args = parser.parse_args()
assert args.steps == 20 and args.variant == "fp16"
assert args.prompt == "a red ceramic teapot on a wooden table"
assert args.gpu_uuid.startswith("GPU-") and args.expected_sku not in ("", "UNBOUND")
assert args.expected_device_name not in ("", "UNBOUND")
assert 7 * 1024**3 <= args.expected_total_bytes <= 8 * 1024**3, "this separate fixture requires an actual 8 GiB-class card"
args.output.mkdir(parents=True, exist_ok=False)
REFERENCE_SHA = "b462aa6b70198e147ad4390dc1524216a44cf2b1dae97664bc21a71f8d4a8749"
reference_path = Path(__file__).with_name("uncapped-reference.npy")
assert hashlib.sha256(reference_path.read_bytes()).hexdigest() == REFERENCE_SHA
reference = np.load(reference_path, allow_pickle=False)
assert reference.shape == (1, 1024, 1024, 3)
census_capture = runpy.run_path(str(Path(__file__).with_name("residency_census.py")))["snapshot"]
census_partition = runpy.run_path(str(Path(__file__).with_name("partition_census.py")))["partition"]
device = torch.device("cuda", args.device)
owner = pipe = None
timings, observations, comparisons = {}, {}, []
stage = "initialization"
failure = None
sampler = sampler_file = None
report = {"diagnostic": "adopted-only-two-call-actual-8gib-hardware", "managed_cap_requested_bytes": None,
          "pid": os.getpid(), "expected_provider_sku": args.expected_sku,
          "provider_sku_scope": "operator-bound quote/hardware receipt; CUDA verifies name, UUID and exact visible memory below",
          "h2d_transferred_bytes": None, "h2d_transfer_calls": None,
          "transfer_counter_note": "Existing native interfaces expose fills/evictions, not actual H2D bytes or calls; no estimate substituted.",
          "reference_sha256": REFERENCE_SHA, "reference_runs": [26551, 26555, 26578],
          "scope": "Actual 8 GiB-class CUDA hardware, CPU construction followed by offload, no full-GPU preload, artificial cap or application retry. Two calls, not a performance benchmark or full Runtime model gate."}

def save(name, value):
    (args.output / name).write_text(json.dumps(value, indent=2) + "\n")

def mark(value):
    global stage
    stage = value
    save("stage.json", {"stage": stage, "at_epoch": time.time()})
    save("timings.json", timings)

def snapshot(label):
    try:
        value = {"working": vt.stats(args.device), "cuda_free_total": list(torch.cuda.mem_get_info(device))}
        if owner is not None:
            census = census_capture(owner)
            save("census-" + label + ".json", census)
            part = census_partition(census)
            save("partition-" + label + ".json", part)
            value["weight_partition"] = {k: part[k] for k in ("groups", "arena_observations", "sampled_mapped_total", "current_unique_regions", "current_unreserved_versions", "finalizing")}
            value["gpu_stats"] = owner._owner.gpu.stats()
            value["weight_event_queries_records_host_waits"] = list(_ext._varena_public_gpu_event_counts(owner._owner.gpu._handle))
            value["combined_managed_observed_bytes"] = value["working"]["mapped_bytes"] + part["sampled_mapped_total"]
            value["mapping_observation_is_nonatomic"] = True
            value["intent_facts"] = owner.intent_facts()
        observations[label] = value
    except Exception as error:
        observations[label] = {"snapshot_error": repr(error)}
    save("observations.json", observations)

def preserve(label, images):
    from PIL import Image
    np.save(args.output / (label + ".npy"), images, allow_pickle=False)
    for index, image in enumerate(images):
        Image.fromarray((np.clip(image, 0, 1) * 255).round().astype(np.uint8)).save(args.output / f"{label}-{index}.webp", lossless=True)
    save("timings.json", timings)

def run():
    with torch.inference_mode():
        return pipe(
            prompt=args.prompt,
            num_inference_steps=args.steps,
            generator=torch.Generator(device=device).manual_seed(1701),
            output_type="np",
        ).images

try:
    vt.install(args.device)
    mark("actual-hardware-preflight")
    properties = torch.cuda.get_device_properties(args.device)
    normalize = lambda value: str(value).removeprefix("GPU-").replace("-", "").lower()
    actual_uuid = str(properties.uuid)
    hardware = {"gpu_uuid": actual_uuid, "device_name": properties.name,
                "total_memory_bytes": properties.total_memory,
                "capability": [properties.major, properties.minor],
                "cuda_visible_devices": os.environ.get("CUDA_VISIBLE_DEVICES"),
                "free_total_before_model": list(torch.cuda.mem_get_info(device))}
    report["hardware"] = hardware
    save("hardware.json", hardware)
    assert normalize(actual_uuid) == normalize(args.gpu_uuid), hardware
    assert properties.name == args.expected_device_name, hardware
    assert properties.total_memory == args.expected_total_bytes, hardware
    report["hardware_verified"] = True
    snapshot("hardware-before-model")
    vt.reset_peak(args.device)
    mark("load-model-on-cpu")
    started = time.perf_counter()
    pipe = StableDiffusionXLPipeline.from_pretrained(args.model, torch_dtype=torch.float16, local_files_only=True, variant=args.variant, use_safetensors=True)
    timings["load_local_model_s"] = time.perf_counter() - started
    assert pipe.vae.config.force_upcast
    assert all(p.device.type == "cpu" for c in pipe.components.values() if isinstance(c, torch.nn.Module) for p in c.parameters())
    ids = {name: [id(p) for p in c.parameters()] for name, c in pipe.components.items() if isinstance(c, torch.nn.Module)}
    mark("offload-adoption")
    started = time.perf_counter()
    owner = vt.offload(pipe, device=args.device)
    timings["offload_setup_call_s"] = time.perf_counter() - started
    snapshot("after-adoption")
    try:
        sampler_file = (args.output / "nvml-samples.csv").open("w")
        sampler = subprocess.Popen(["nvidia-smi", "--query-gpu=timestamp,uuid,memory.used,memory.total,utilization.gpu", "--format=csv,noheader,nounits", "-i", str(args.device), "-lms", "200"], stdout=sampler_file, stderr=subprocess.DEVNULL)
    except OSError as error:
        report["nvml_sampler_error"] = repr(error)
    for index in range(2):
        label = f"adopted-{index + 1}"
        snapshot(label + "-before")
        mark(label + "-generation")
        started = time.perf_counter()
        try:
            images = run()
        finally:
            timings[label + "_generation_to_numpy_s"] = time.perf_counter() - started
            save("timings.json", timings)
            snapshot(label + "-after")
        preserve(label, images)
        assert pipe.vae.config.force_upcast and pipe.vae.dtype == torch.float16
        assert ids == {name: [id(p) for p in c.parameters()] for name, c in pipe.components.items() if isinstance(c, torch.nn.Module)}
        verdict = {"call": index + 1, "rtol": 1e-3, "atol": 1e-3, "shape": list(images.shape), "max_abs_error": float(np.max(np.abs(images - reference)))}
        try:
            np.testing.assert_allclose(images, reference, rtol=1e-3, atol=1e-3)
        except AssertionError as error:
            verdict.update(passed=False, original_assertion=str(error))
        else:
            verdict["passed"] = True
        comparisons.append(verdict)
        save("comparisons.json", comparisons)
    mark("complete")
except BaseException as error:
    failure = error
    report["failure"] = {"stage": stage, "type": type(error).__name__, "message": str(error), "typed_capacity": isinstance(error, (torch.OutOfMemoryError, api.Exhausted)), "traceback": traceback.format_exc()}
    save("failure.json", report["failure"])
    snapshot("failure")
finally:
    if sampler is not None:
        if sampler.poll() is None:
            sampler.terminate()
            try:
                sampler.wait(timeout=5)
            except subprocess.TimeoutExpired:
                sampler.kill()
                sampler.wait()
        report["nvml_sampler_returncode"] = sampler.returncode
    if sampler_file is not None:
        sampler_file.close()
    samples = []
    if (args.output / "nvml-samples.csv").exists():
        for row in csv.reader((args.output / "nvml-samples.csv").read_text().splitlines()):
            try:
                samples.append({"timestamp": row[0].strip(), "gpu_uuid": row[1].strip(), "memory_used_mib": int(row[2]), "memory_total_mib": int(row[3]), "gpu_util_percent": int(row[4])})
            except (ValueError, IndexError):
                pass
    report["physical_memory_samples"] = {"count": len(samples), "sampled_peak_used_mib": max((x["memory_used_mib"] for x in samples), default=None), "period_ms": 200, "includes_context_and_library_allocations": True, "exact_peak": False}
    if owner is not None:
        try:
            owner.close()
        except Exception as error:
            report["cleanup_error"] = repr(error)
    report.update(passed=failure is None and "cleanup_error" not in report and len(comparisons) == 2 and all(x["passed"] for x in comparisons), steps=args.steps, variant=args.variant, seed=1701, completed_calls=len(comparisons), timings=timings, comparisons=comparisons, stage=stage)
    report["output_files"] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(args.output.iterdir()) if p.is_file()}
    save("hardware-result.json", report)
    print(json.dumps(report), flush=True)
if failure is not None:
    raise failure
assert report["passed"], comparisons
