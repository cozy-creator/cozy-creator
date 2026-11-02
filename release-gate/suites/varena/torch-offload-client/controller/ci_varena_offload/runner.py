"""Fresh stdlib-only launcher; the frozen client owns Torch/Varena installation."""

import base64
import contextlib
import csv
import hashlib
import importlib.metadata
import io
import json
import os
import runpy
import sys
from pathlib import Path


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def uuid_key(value):
    return value.lower().removeprefix("gpu-").removeprefix("mig-").replace("-", "")


payload = json.loads(Path(sys.argv[1]).read_text())
case = sys.argv[2]
assert case in ("intent", "adapter", "stock_sdxl", "stock_sdxl_census")
stock = case in ("stock_sdxl", "stock_sdxl_census")
if case == "stock_sdxl_census":
    assert payload["steps"] == 20
artifact_dir = Path(sys.argv[3])
assert payload["gpu_uuid"] != "UNBOUND"
assert "torch" not in sys.modules and "tensorfs" not in sys.modules
binaries = Path(__file__).with_name("bin")
manifest = json.loads((binaries / "manifest.json").read_text())
assert manifest["source_sha"] == payload["source_sha"]
assert manifest["script_source_sha"] == payload["script_source_sha"]
assert manifest["scripts"] == payload["scripts"]
for name, expected in payload["scripts"].items():
    assert digest(binaries / name) == expected, name
# Metadata/file inspection must not import either frontend or initialize CUDA.
distribution = importlib.metadata.distribution("tensorfs")
assert distribution.version == payload["tensorfs_version"]
root = Path(distribution.locate_file("tensorfs"))
for name, expected in payload["tensorfs_files"].items():
    assert digest(root / name) == expected, name
extensions = list(root.glob("_ext*.so"))
assert len(extensions) == 1 and digest(extensions[0]) == payload["extension_sha256"]
assert digest(root / "_varena_torch.so") == payload["shim_sha256"]
assert digest(root / ".native/libvarena.so") == payload["core_sha256"]
assert digest(root / ".native" / payload["std_name"]) == payload["std_sha256"]
assert "torch" not in sys.modules and "tensorfs" not in sys.modules
# Resolve package metadata without importing Torch, TensorFS or Diffusers.
package_facts = {}
for name, expected in payload["package_versions"].items():
    dist = importlib.metadata.distribution(name)
    assert dist.version == expected, (name, dist.version, expected)
    package_facts[name] = {
        "version": dist.version,
        "metadata_sha256": hashlib.sha256(
            dist.read_text("METADATA").encode()
        ).hexdigest(),
    }
model_files = []
if stock:
    layout = json.loads((binaries / "model-layout.json").read_text())
    assert layout["revision"] == "462165984030d82259a11f4367a4eed129e94a7b"
    assert payload["variant"] == layout["variant"] == "fp16"
    model = Path(payload["model_path"])
    assert model.is_absolute() and model.is_dir()
    for item in layout["files"]:
        relative = Path(item["path"])
        assert not relative.is_absolute() and ".." not in relative.parts
        path = model / relative
        assert path.stat().st_size == item["bytes"], str(path)
        if item["sha256"] is not None:
            actual = digest(path)
            assert actual == item["sha256"], str(path)
            algorithm = "sha256"
        else:
            data = path.read_bytes()
            actual = hashlib.sha1(
                b"blob " + str(len(data)).encode() + b"\0" + data
            ).hexdigest()
            assert actual == item["blob_id"], str(path)
            algorithm = "git-blob-sha1"
        model_files.append(
            {
                "path": item["path"],
                "bytes": item["bytes"],
                "algorithm": algorithm,
                "digest": actual,
            }
        )
    assert (
        len(model_files) == 18 and sum(x["bytes"] for x in model_files) == 6_941_187_536
    )
script = {
    "intent": "intent.py",
    "adapter": "adapter_gpu.py",
    "stock_sdxl": "sdxl_gpu.py",
    "stock_sdxl_census": "sdxl_census.py",
}[case]
sys.argv = [str(binaries / script), "--device", "0"]
if stock:
    sys.argv += [
        payload["model_path"],
        "--variant",
        payload["variant"],
        "--steps",
        str(payload["steps"]),
        "--prompt",
        payload["prompt"],
        "--output",
        str(artifact_dir),
    ]
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
assert "torch" not in sys.modules and "tensorfs" not in sys.modules
captured = io.StringIO()


class Tee:
    def write(self, text):
        sys.__stdout__.write(text)
        captured.write(text)
        return len(text)

    def flush(self):
        sys.__stdout__.flush()


# Original clients own offload/install before actual CUDA use. No wrapper warm-up.
with contextlib.redirect_stdout(Tee()):
    retained_namespace = runpy.run_path(str(binaries / script), run_name="__main__")
rows = [
    json.loads(line)
    for line in captured.getvalue().splitlines()
    if line.startswith("{")
]
assert rows, "original client produced no structured result"
value = rows[-1]
import torch
from tensorfs import _ext
from tensorfs.varena import api

assert torch.__version__ == "2.14.0+cu130"
assert torch.version.git_version == "08187d9e0fba026dc8217405802ab5381dc88d90"

# A seeded SDK may symlink individual files. Resolving __init__.py before
# taking its parent then names the cache directory, not the installed package.
# Validate the actual loaded files against the exact prechecked distribution
# entries, preserving inode identity and every content hash.
python_files = {}
for relative, expected in payload["tensorfs_files"].items():
    module_name = "tensorfs." + relative.removesuffix(".py").replace("/", ".")
    module_name = module_name.removesuffix(".__init__")
    module = sys.modules.get(module_name)
    if module is None:
        continue
    loaded = Path(module.__file__)
    installed = root / relative
    facts = {
        "loaded": str(loaded),
        "resolved": str(loaded.resolve()),
        "installed": str(installed),
        "installed_resolved": str(installed.resolve()),
    }
    assert loaded.samefile(installed) and digest(loaded) == expected, facts
    assert module.__spec__ is not None and Path(module.__spec__.origin).samefile(
        installed
    ), facts
    python_files[relative] = facts
assert "__init__.py" in python_files and "varena/api.py" in python_files
assert Path(_ext.__file__).samefile(extensions[0])
assert digest(_ext.__file__) == payload["extension_sha256"]
uuid = str(torch.cuda.get_device_properties(0).uuid)
assert uuid_key(uuid) == uuid_key(payload["gpu_uuid"])
identities = [_ext._varena_core_identity(), api.core_identity()]
assert identities[0] and len(set(identities)) == 1
mapped = {
    line.split(maxsplit=5)[-1]
    for line in Path("/proc/self/maps").read_text().splitlines()
    if len(line.split(maxsplit=5)) == 6
}
std_names = {
    Path(path).name
    for path in mapped
    if Path(path).name.startswith("libstd-") and path.endswith(".so")
}
assert std_names == {payload["std_name"]}, std_names
libraries = {}
for name, expected in (
    ("libvarena.so", payload["core_sha256"]),
    (payload["std_name"], payload["std_sha256"]),
):
    paths = {path for path in mapped if Path(path).name == name}
    assert len(paths) == 1, (name, paths)
    path = next(iter(paths))
    assert digest(path) == expected, path
    libraries[name] = path
# Verify actual imported framework/package files against their installed wheel
# RECORD entries; package versions are exact and the prepared uv.lock pins wheels.
file_manifest = {}
for distribution_name, package in [
    ("torch", "torch"),
    ("diffusers", "diffusers"),
    ("transformers", "transformers"),
]:
    dist = importlib.metadata.distribution(distribution_name)
    records = {
        row[0]: row[1]
        for row in csv.reader(io.StringIO(dist.read_text("RECORD")))
        if len(row) >= 2
    }
    loaded_files = {}
    for name, module in list(sys.modules.items()):
        if name != package and not name.startswith(package + "."):
            continue
        path = getattr(module, "__file__", None)
        if path is None or not path.endswith(".py"):
            continue
        relative = name.replace(".", "/") + (
            "/__init__.py" if hasattr(module, "__path__") else ".py"
        )
        if relative not in records:
            continue
        installed = Path(dist.locate_file(relative))
        assert Path(path).samefile(installed), (name, path, str(installed))
        encoding, encoded = records[relative].split("=", 1)
        assert encoding == "sha256"
        actual = digest(path)
        assert (
            base64.urlsafe_b64encode(bytes.fromhex(actual)).rstrip(b"=").decode()
            == encoded
        ), name
        loaded_files[relative] = actual
    file_manifest[distribution_name] = loaded_files
    package_facts[distribution_name]["loaded_file_count"] = len(loaded_files)
    package_facts[distribution_name]["loaded_files_sha256"] = hashlib.sha256(
        json.dumps(loaded_files, sort_keys=True).encode()
    ).hexdigest()
artifact_dir.mkdir(parents=True, exist_ok=True)
(artifact_dir / "package-file-identity.json").write_text(
    json.dumps(file_manifest, indent=2) + "\n"
)
if case == "intent":
    assert value["gpu_executed"] is True and value["observations"]
    assert all(x["native"] and x["generation"] > 0 for x in value["observations"])
    assert len(value["declared"]["regions"]) == 6
    scope = (
        "actual nested/repeated native intent, exception retirement and dtype refresh"
    )
elif case == "adapter":
    assert value["tied_direct_alias_streams"] == value["tiny_vae"] == "pass"
    assert value["full_sdxl"] == "not exercised"
    scope = "actual tied/direct/alias/stream and tiny stock VAE consumers; no full SDXL claim"
else:
    assert (
        value["stock_sdxl_vae_cycle"] == "pass" and value["steps"] == payload["steps"]
    )
    assert (
        value["variant"] == payload["variant"]
        and value["performance_qualification"] is False
    )
    for name, expected in value["output_files"].items():
        assert digest(artifact_dir / name) == expected
    scope = (
        "2-step correctness smoke only"
        if payload["steps"] == 2
        else "single matched20-step stock baseline/offload comparison; no performance qualification"
    )
    if case == "stock_sdxl_census":
        assert value["diagnostic"] == "fixed-two-pair-residency-census"
        assert value["census_snapshots"] == 5
        scope = "fixed two baseline/two adopted20-step residency diagnostic; no performance qualification"
print(
    json.dumps(
        {
            "passed": True,
            "case": case,
            "pid": os.getpid(),
            "gpu_uuid": uuid,
            "scope": scope,
            "original_result": value,
            "core_identities": identities,
            "mapped_libraries": libraries,
            "python_file_identity": python_files,
            "package_facts": package_facts,
            "model_files": model_files,
            "source_sha": payload["source_sha"],
            "script_source_sha": payload["script_source_sha"],
            "script_sha256": digest(binaries / script),
        },
        sort_keys=True,
    )
)
