"""Stage the existing stock-pressure controller for actual 8 GiB hardware; local files only."""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import shutil
import subprocess
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def once(text, before, after):
    assert text.count(before) == 1, before
    return text.replace(before, after, 1)


def git(repo, *arguments):
    return subprocess.check_output(["git", "-C", str(repo), *arguments])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=Path, required=True)
    parser.add_argument("--creator", type=Path, required=True)
    parser.add_argument("--creator-ref", required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--runtime-ref", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    original = args.seed / "project/ci_varena_offload"
    old_script = original / "bin/sdxl_pressure.py"
    assert digest(old_script) == "f91bd33f1fe8deff8fc03265d269986242aeb99c3f298983d46d0c32a79734e4"
    fixture = here / "sdxl_hardware.py"
    def run_ast(path):
        tree = ast.parse(path.read_text())
        return ast.dump(next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "run"), include_attributes=False)
    assert run_ast(fixture) == run_ast(old_script)
    creator = git(args.creator, "rev-parse", args.creator_ref).decode().strip()
    runtime = git(args.runtime, "rev-parse", args.runtime_ref).decode().strip()
    args.output.mkdir(parents=True, exist_ok=False)
    project = args.output / "project"
    package = project / "ci_varena_offload"
    binaries = package / "bin"
    binaries.mkdir(parents=True)
    for name in ("pyproject.toml", "uv.lock"):
        shutil.copy2(args.seed / "project" / name, project / name)
    # Keep the original package identity and execution custody; expose only its existing
    # stock_pressure entrypoint in this separately named source project.
    parent = (original / "__init__.py").read_text()
    parent = once(parent, "    gpu_uuid: str\n", "    gpu_uuid: str\n    expected_sku: str\n    expected_device_name: str\n    expected_total_bytes: int\n")
    lines = parent.splitlines(keepends=True)
    remove = set()
    for node in ast.parse(parent).body:
        if isinstance(node, ast.FunctionDef) and node.decorator_list and node.name != "stock_pressure":
            first = min(d.lineno for d in node.decorator_list)
            remove.update(range(first - 1, node.end_lineno))
    parent = "".join(line for i, line in enumerate(lines) if i not in remove)
    parent = parent.replace('"""Ordinary Cozy custody for the unchanged offload GPU clients."""', '"""Existing child custody for the separate actual-hardware stock SDXL fixture."""')
    (package / "__init__.py").write_text(parent)
    runner = (original / "runner.py").read_text()
    runner = once(runner, 'assert case in ("intent", "adapter", "parts", "packing", "stock_sdxl", "stock_sdxl_census", "plan_retirement", "stock_pressure") or case in ready_modes', 'assert case == "stock_pressure", "this prepared project qualifies only actual-hardware SDXL"')
    runner = once(runner, '"stock_pressure": "sdxl_pressure.py",', '"stock_pressure": "sdxl_hardware.py",')
    runner = once(runner, 'os.environ["HF_HUB_OFFLINE"] = "1"', '''sys.argv += ["--gpu-uuid", payload["gpu_uuid"],
             "--expected-sku", payload["expected_sku"],
             "--expected-device-name", payload["expected_device_name"],
             "--expected-total-bytes", str(payload["expected_total_bytes"])]
os.environ["HF_HUB_OFFLINE"] = "1"''')
    runner = once(runner, '    assert value["managed_cap_requested_bytes"] == 6 * 1024**3 and value["cap_probe"]["passed"]\n    assert value["diagnostic"] == "adopted-only-two-call-managed-6gib-pressure"', '''    assert value["managed_cap_requested_bytes"] is None and value["hardware_verified"]
    assert value["diagnostic"] == "adopted-only-two-call-actual-8gib-hardware"
    assert value["hardware"]["total_memory_bytes"] == payload["expected_total_bytes"]
    assert value["hardware"]["device_name"] == payload["expected_device_name"]
    assert uuid_key(value["hardware"]["gpu_uuid"]) == uuid_key(payload["gpu_uuid"])
    assert value["expected_provider_sku"] == payload["expected_sku"]''')
    runner = once(runner, '    scope = "adopted-only2calls under6GiBmanagedcap withtypedoversizeprobe; sameuncappedreference; notactual6/8GBGPUproof"', '    scope = "actual8GiBhardware adopted-only two-call SDXL; same external reference; no managed cap, application retry or full Runtime model claim"')
    (package / "runner.py").write_text(runner)
    origins = {}
    for name in ("residency_census.py", "partition_census.py"):
        relative = "release-gate/suites/varena/torch-offload/" + name
        data = git(args.creator, "show", f"{creator}:{relative}")
        assert data == (original / "bin" / name).read_bytes(), name
        (binaries / name).write_bytes(data)
        origins[name] = {"repository": "cozy-creator", "commit": creator, "path": relative}
    shutil.copy2(fixture, binaries / fixture.name)
    for name in ("model-layout.json", "uncapped-reference.npy"):
        shutil.copy2(original / "bin" / name, binaries / name)
        origins[name] = {"path": str(original / "bin" / name), "sha256": digest(original / "bin" / name)}
    assert digest(binaries / "uncapped-reference.npy") == "b462aa6b70198e147ad4390dc1524216a44cf2b1dae97664bc21a71f8d4a8749"
    payload = json.loads((args.seed / "stock_pressure-input.json").read_text())
    for name in ("gpu_uuid", "tensorfs_version", "runtime_version", "extension_sha256", "shim_sha256", "core_sha256", "std_name", "std_sha256", "model_path"):
        payload[name] = "UNBOUND"
    for name in payload["tensorfs_files"]:
        data = git(args.creator, "show", f"{creator}:crates/tensorfs-py/python/tensorfs/{name}")
        payload["tensorfs_files"][name] = hashlib.sha256(data).hexdigest()
    for name in payload["runtime_files"]:
        data = git(args.runtime, "show", f"{runtime}:src/cozy_runtime/{name}")
        payload["runtime_files"][name] = hashlib.sha256(data).hexdigest()
    payload.update(source_sha=creator, script_source_sha=digest(fixture), expected_sku="UNBOUND", expected_device_name="UNBOUND", expected_total_bytes=0)
    payload["scripts"] = {p.name: digest(p) for p in sorted(binaries.iterdir()) if p.is_file()}
    manifest = {"status": "UNBOUND", "source_sha": creator, "runtime_source": runtime,
                "script_source_sha": digest(fixture), "scripts": payload["scripts"], "script_origins": origins,
                "package_versions": payload["package_versions"], "lock_sha256": digest(project / "uv.lock"),
                "original_pressure_sha256": digest(old_script), "original_manifest_sha256": digest(original / "bin/manifest.json"),
                "run_ast_unchanged": True, "gpu_executed": False}
    (binaries / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / "stock-hardware-input.json").write_text(json.dumps(payload, indent=2) + "\n")
    receipt = {**manifest, "project": str(project), "source_seed": str(args.seed),
               "original_wrapper_hashes": {name: digest(original / name) for name in ("__init__.py", "runner.py")},
               "prepared_wrapper_hashes": {name: digest(package / name) for name in ("__init__.py", "runner.py")}}
    for p in project.rglob("*.py"):
        compile(p.read_text(), str(p), "exec")
    (args.output / "preparation.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "status": "UNBOUND", "run_ast_unchanged": True}))


if __name__ == "__main__":
    main()
