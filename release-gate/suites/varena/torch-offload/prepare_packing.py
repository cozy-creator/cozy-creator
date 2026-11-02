"""Prepare UNBOUND ordinary-Cozy packing children from a preserved proven seed.

This only copies/hashes local fixture files. It does not install, build, download,
rent, run a child, or bind any candidate/GPU identities.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def replace_once(text, before, after):
    assert text.count(before) == 1, before
    return text.replace(before, after, 1)


def prepare(seed, output, source_ref):
    here = Path(__file__).resolve().parent
    repo = Path(subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], cwd=here, text=True
    ).strip())
    fixture_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=repo, text=True
    ).strip()
    assert not output.exists(), "use a new preparation directory"
    seed_bin = seed / "project/ci_varena_offload/bin"
    original = json.loads((seed_bin / "manifest.json").read_text())
    for name, expected in original["scripts"].items():
        assert sha(seed_bin / name) == expected, name
    assert sha(seed / "project/uv.lock") == original["lock_sha256"]
    output.mkdir(parents=True)
    shutil.copytree(seed, output / "upstream-original")
    project = output / "project"
    shutil.copytree(seed / "project", project)
    package = project / "ci_varena_offload"
    binaries = package / "bin"
    origins = {}
    for relative in (
        "release-gate/suites/varena/native-tensor-export/parts_gpu.py",
        "release-gate/suites/varena/torch-offload/packing.py",
    ):
        data = subprocess.check_output(["git", "show", f"{source_ref}:{relative}"], cwd=repo)
        (binaries / Path(relative).name).write_bytes(data)
        origins[Path(relative).name] = {"source_sha": source_ref, "path": relative}
    for name in ("residency_census.py", "partition_census.py", "sdxl_census.py"):
        shutil.copyfile(here / name, binaries / name)
        origins[name] = {"source_sha": fixture_ref, "path": str((here / name).relative_to(repo))}
    assert sha(binaries / "sdxl_census.py") == original["scripts"]["sdxl_census.py"]

    runner = (package / "runner.py").read_text()
    runner = replace_once(
        runner,
        'assert case in ("intent", "adapter", "stock_sdxl", "stock_sdxl_census")',
        'assert case in ("intent", "adapter", "parts", "packing", "stock_sdxl", "stock_sdxl_census")',
    )
    runner = replace_once(
        runner, '    "intent": "intent.py",',
        '    "parts": "parts_gpu.py",\n    "packing": "packing.py",\n    "intent": "intent.py",',
    )
    runner = replace_once(
        runner, '# Metadata/file inspection must not import either frontend or initialize CUDA.',
        '''# Exact Runtime identity is checked in both parent and isolated child.
runtime_distribution = importlib.metadata.distribution("cozy-runtime")
assert runtime_distribution.version == payload["runtime_version"]
runtime_root = Path(runtime_distribution.locate_file("cozy_runtime"))
for name, expected in payload["runtime_files"].items():
    assert digest(runtime_root / name) == expected, name
# Metadata/file inspection must not import either frontend or initialize CUDA.''',
    )
    runner = replace_once(
        runner, 'if case == "intent":\n    assert value["gpu_executed"]',
        '''if case == "parts":
    assert len(value["checks"]) == 5 and value["device"] == 0
    assert value["native_stats"]["weights_resident"] == 0
    scope = "packed immutable Sources, typed Pins, fixed-address refill and warm graph replay"
elif case == "packing":
    assert value["native_executed"] is True and value["metadata_doubles"] is False
    assert value["device"] == "cuda:0" and len(value["checks"]) == 7
    scope = "common offload packing, direct reads, aliases, real conversion refusals and trim/refill"
elif case == "intent":
    assert value["gpu_executed"]''',
    )
    (package / "runner.py").write_text(runner)
    parent = (package / "__init__.py").read_text()
    parent = replace_once(
        parent, '        with tempfile.TemporaryDirectory(prefix="offload-client-") as folder:',
        '''        runtime_root = Path(importlib.metadata.distribution("cozy-runtime").locate_file("cozy_runtime"))
        runtime_identity = {}
        for relative, expected in payload.runtime_files.items():
            name = "cozy_runtime." + relative.removesuffix(".py").replace("/", ".")
            module = sys.modules.get(name.removesuffix(".__init__"))
            if module is not None:
                loaded, installed = Path(module.__file__), runtime_root / relative
                assert loaded.samefile(installed), (str(loaded), str(installed))
                assert hashlib.sha256(loaded.read_bytes()).hexdigest() == expected
                runtime_identity[relative] = {"loaded": str(loaded), "installed": str(installed)}
        result["parent"]["runtime_file_identity"] = runtime_identity
        with tempfile.TemporaryDirectory(prefix="offload-client-") as folder:''',
    )
    parent += '''

@app.entrypoint(accelerator=True)
def parts(payload: Probe, ctx: Context, out: Outputs) -> Report:
    return execute(payload, ctx, "parts", out)


@app.entrypoint(accelerator=True)
def packing(payload: Probe, ctx: Context, out: Outputs) -> Report:
    return execute(payload, ctx, "packing", out)
'''
    (package / "__init__.py").write_text(parent)
    scripts = {
        path.name: sha(path)
        for path in sorted(binaries.iterdir())
        if path.is_file() and path.name != "manifest.json"
    }
    manifest = {
        "status": "UNBOUND",
        "source_sha": source_ref,
        "script_source_sha": fixture_ref,
        "scripts": scripts,
        "script_origins": origins,
        "upstream_manifest_sha256": sha(seed_bin / "manifest.json"),
        "lock_sha256": sha(project / "uv.lock"),
        "package_versions": original["package_versions"],
        "candidate": "UNBOUND",
    }
    (binaries / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    payload = json.loads((seed / "input.json").read_text())
    for key in (
        "gpu_uuid", "tensorfs_version", "runtime_version", "extension_sha256",
        "shim_sha256", "core_sha256", "std_name", "std_sha256", "model_path",
    ):
        payload[key] = "UNBOUND"
    for key in ("tensorfs_files", "runtime_files"):
        payload[key] = dict.fromkeys(payload[key], "UNBOUND")
    payload.update(source_sha=source_ref, script_source_sha=fixture_ref, scripts=scripts)
    for name in ("parts", "packing", "stock-census-20"):
        payload["steps"] = 20 if name == "stock-census-20" else 2
        (output / f"{name}-input.json").write_text(json.dumps(payload, indent=2) + "\n")
    ready = {
        "status": "UNBOUND", "candidate": "UNBOUND", "gpu_uuid": "UNBOUND",
        "source_sha": source_ref, "fixture_sha": fixture_ref,
        "sequence": ["parts", "packing", "stock_sdxl_census"],
        "project": str(project), "manifest": manifest,
        "wrapper_sha256": {name: sha(package / name) for name in ("__init__.py", "runner.py")},
        "preserved_seed": str(output / "upstream-original"),
        "numerical_and_timing_script_unchanged": True,
        "gpu_executed": False,
    }
    (output / "ready.json").write_text(json.dumps(ready, indent=2) + "\n")
    print(json.dumps({"status": "UNBOUND", "ready": str(output / "ready.json")}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-ref", required=True)
    args = parser.parse_args()
    prepare(args.seed.resolve(), args.output.resolve(), args.source_ref)
