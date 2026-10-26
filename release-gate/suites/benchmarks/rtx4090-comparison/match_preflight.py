# /// script
# requires-python = ">=3.12"
# [tool.cozy]
# accelerator = false
# ///
"""Compare real CPU checkpoint receipts and choose the existing Comfy sigma input.

Usage: python sdxl_preflight.py cozy-identity.json comfy-identity.json matched.json
No fabricated tensor observations, GPU execution or unit-test fixture is accepted.
"""
from __future__ import annotations
import hashlib
import json
import math
import sys
from pathlib import Path
import subprocess
from cozy_runtime.author import Context, FileAsset, Outputs

EXPECTED_CHECKPOINT = "sha256:cbacec84cee812314251badfa5810bedcc31f143e4a874cc1323ae14bd480667"
EXPECTED_REPO_COMMIT = "sha256:df8494606a805c233a46b931174b9388208eb4ea0c83deb23fa833375d2956f5"
COMFY = "fb3befd168731438f88bc96e0920a538e8649878"


def compare(cozy: dict, comfy: dict) -> dict:
    assert cozy["manifest"] == EXPECTED_CHECKPOINT
    assert comfy["comfy_commit"] == COMFY
    assert cozy["torch"] == comfy["torch"] == "2.14.1+cu130"
    assert cozy["torch_git"] and cozy["torch_git"] == comfy["torch_git"]
    assert set(cozy["components"]) == {"unet", "vae", "text_encoder", "text_encoder_2"}
    assert cozy["components"] == comfy["components"], "real normalized tensor bytes/layouts differ"
    assert all(v["dtype"] == "F16" for rows in cozy["components"].values() for v in rows.values())
    a, b = cozy["sigmas"], comfy["sigmas"]
    assert len(a) == len(b) == 31
    for sigmas in (a, b):
        assert all(math.isfinite(x) and x >= 0 for x in sigmas)
        assert sigmas[-1] == 0 and all(x > y for x, y in zip(sigmas, sigmas[1:]))
    natural_matches = all(math.isclose(x, y, rel_tol=1e-5, abs_tol=2e-5)
                          for x, y in zip(a, b, strict=True))
    initial = cozy["init_noise_sigma"]
    assert isinstance(initial, (int, float)) and math.isfinite(initial) and initial > 0
    comfy_initial = comfy.get("init_noise_sigma")
    initial_matches = isinstance(comfy_initial, (int, float)) and math.isclose(initial, comfy_initial, rel_tol=1e-6, abs_tol=1e-6)
    return {
        "weights_match": True,
        "actual_gpu_execution_proven": False,
        "repository_commit": EXPECTED_REPO_COMMIT,
        "tensorfs_checkpoint": EXPECTED_CHECKPOINT,
        "comfy_commit": COMFY,
        "torch": cozy["torch"],
        "torch_git": cozy["torch_git"],
        "steps": 30,
        "natural_comfy_sigmas_match": natural_matches,
        "natural_sigma_max_abs_difference": max(abs(x-y) for x,y in zip(a,b,strict=True)),
        "cozy_sigmas": a,
        "natural_comfy_sigmas": b,
        "comfy_scheduler": "normal" if natural_matches and initial_matches else "manual-cozy-observed",
        "natural_comfy_init_noise_scale_observed": comfy_initial,
        "natural_comfy_initial_scale_match": initial_matches,
        "tensor_counts": {k: len(v) for k,v in cozy["components"].items()},
        "checkpoint_file_sha256": comfy["checkpoint_sha256"],
        "cozy_scheduler_config": cozy["scheduler_config"],
        "cozy_timesteps": cozy["timesteps"],
        "cozy_init_noise_sigma": initial,
        "limitations": [
            "Preflight reads warm both engines' file pages; first inference is not disk-cold.",
            "A shared step grid is not pixel identity: Cozy draws CPU FP16 noise; Comfy draws CPU F32 noise.",
            "Manual mode explicitly feeds Cozy initial-noise scale via existing RandomNoise/AddNoise nodes; the Euler implementation is unchanged.",
            "Both use their real supported Euler/model implementations; rounding and kernels can differ.",
            "SDXL Cozy output is lossless WebP, Comfy SaveImage PNG; separate encoding and complete delivery latency.",
        ],
    }



def main(ctx: Context, *, out: Outputs) -> FileAsset:
    base = Path("/var/tmp/bench4090-20261026")
    inputs = [base / "cozy-identity.json", base / "comfy-identity.json"]
    result = compare(*(json.loads(path.read_text()) for path in inputs))
    result["matched"] = result["weights_match"]
    result["gpu_identity"] = subprocess.check_output(
        ["nvidia-smi", "--query-gpu=uuid,pci.bus_id,name,driver_version", "--format=csv,noheader"], text=True).strip()
    assert len(result["gpu_identity"].splitlines()) == 1
    result["receipt_sha256"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs}
    path = base / "matched-sdxl.json"
    path.write_text(json.dumps(result, indent=2) + "\n")
    return out.save_file(path, media_type="application/json")
