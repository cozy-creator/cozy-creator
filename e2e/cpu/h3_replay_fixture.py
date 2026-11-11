"""Artifact-owned H3 control: unchanged cells, retained checkpoints and one retained input."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


artifact = Path(os.environ["H3_REPLAY_ARTIFACT"]).resolve(strict=True)
source = Path(os.environ["H3_REPLAY_ORIGINAL_FIXTURE"]).resolve(strict=True)
if digest(source) != "d56dcba465ae763299455156d48da2d66d938415612458f8131f76cea0fcbcd1":
    raise ValueError("the original fixture is not the frozen control source")
spec = importlib.util.spec_from_file_location("h3_frozen_fixture", source)
if spec is None or spec.loader is None:
    raise ValueError("the frozen fixture could not be loaded")
original = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = original
spec.loader.exec_module(original)

video = artifact / "inputs/regenerate_shot-native/video.mp4"
if digest(video) != "a288b49c8432886b78923359c20fa6459d586734f001b6b0ccfdf57b5c9f9291":
    raise ValueError("the input is not run43061139019's retained rank-1 video")
WAIVE, PATCH = original.WAIVE, original.PATCH
CELLS = [{**cell, "files": {**cell.get("files", {}), "video": video}}
         for cell in original.CELLS
         if cell["entry"] == "regenerate_shot" and cell["name"] in {"native", "native-2-ranks"}]
if [cell["name"] for cell in CELLS] != ["native", "native-2-ranks"]:
    raise ValueError("the original rank pair changed")

_HEADERS = {
    original.H3: ("60dd4458fb0d4ba3c33827c39079a2aa3cbe11948fae990edae50ecd64ee49d2",
                  {"text_encoder", "vae", "audio_vae", "transformer", "transformer_ref"}),
    "alibaba-pai/minimax-h3-turbo-lora@1.0.1": (
        "cd1c10bd46d034b923023291f6f7e290b8e44bef4378f8ab4094e0ebd9b0f633", {"fl2va_turbo", "ref2va_turbo"}),
    original.REGENERATE: ("abb42c762bba45d08e3fe04d6496e1d81ee3f9e8da1728c9ffc479b9fda67e82", {"fl2va_regenerate"}),
    original.UPSCALER: ("2eb91c71ae5c9521ced7987fbf8748113fc6f61263910d8fc193191735a57a93", {"upscaler"}),
}


def models(root: Path, store: Path) -> dict[str, Path | str]:
    import tensorfs

    store.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(artifact / "store/format", store / "format")
    shutil.copytree(artifact / "store/blobs", store / "blobs", dirs_exist_ok=True)
    manifests = {}
    for ref, (header, components) in _HEADERS.items():
        path = store / "blobs" / header[:2] / header[2:4] / header
        if digest(path) != header:
            raise ValueError(f"retained checkpoint header differs: {ref}")
        manifest = "sha256:" + header
        checkpoint = tensorfs.open(manifest, cache_dir=store, local_files_only=True)
        if set(checkpoint.components) != components:
            raise ValueError(f"retained checkpoint components differ: {ref}")
        manifests[ref] = manifest
    (store.parent / "retained-input-control.json").write_text(json.dumps({
        "source_run": "43061139019", "input_sha256": digest(video),
        "fixture_sha256": digest(source), "manifests": manifests,
    }, indent=2) + "\n")
    return manifests
