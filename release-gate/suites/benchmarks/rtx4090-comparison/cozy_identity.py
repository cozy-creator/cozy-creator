# /// script
# requires-python = ">=3.12"
# dependencies = ["diffusers==0.40.0", "torch==2.14.1", "numpy"]
# [tool.cozy]
# accelerator = false
# ///
"""Untimed real checkpoint identity/read preflight through the ordinary Weights grant."""
import hashlib
import json
from pathlib import Path

from cozy_runtime.author import Context, FileAsset, Outputs, Weights


def main(
    weights: Weights, ctx: Context, *, out: Outputs,
    expected_runtime_source: str = "774624133be6d74262f845b9358874872de100d9",
) -> FileAsset:
    import torch
    from diffusers import EulerDiscreteScheduler
    from cozy_runtime._build_provenance import COMMIT
    import tensorfs

    assert len(expected_runtime_source) == 40 and all(c in "0123456789abcdef" for c in expected_runtime_source), expected_runtime_source
    assert COMMIT == expected_runtime_source, (COMMIT, expected_runtime_source)

    source = ctx.tensorfs_source(weights)
    assert source.id == "sha256:cbacec84cee812314251badfa5810bedcc31f143e4a874cc1323ae14bd480667"
    rows = {}
    for name in ("unet", "vae", "text_encoder", "text_encoder_2"):
        component = source.components[name]
        rows[name] = {}
        for key, metadata in component.tensors.items():
            ctx.raise_if_cancelled()
            assert metadata.encoding is None, (name, key, metadata.encoding)
            value = component.tensors.read(key)
            rows[name][key] = {"dtype": value.dtype, "shape": list(value.shape),
                              "sha256": hashlib.sha256(value.tobytes()).hexdigest()}
    scheduler_config = source.components["scheduler"].config
    schedule = EulerDiscreteScheduler.from_config(scheduler_config)
    schedule.set_timesteps(30, device="cpu")
    receipt = {"manifest": source.id, "runtime_commit": COMMIT, "tensorfs": tensorfs.__version__, "torch": torch.__version__, "torch_git": torch.version.git_version,
               "components": rows, "scheduler_config": scheduler_config,
               "init_noise_sigma": float(schedule.init_noise_sigma),
               "sigmas": schedule.sigmas.tolist(), "timesteps": schedule.timesteps.tolist(),
               "scope": "CPU-only logical tensor byte identity; this read warms source OS pages. No GPU allocation or inference."}
    path = Path("/var/tmp/bench4090-20261026/cozy-identity.json")
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(receipt, sort_keys=True))
    return out.save_file(path, media_type="application/json")
