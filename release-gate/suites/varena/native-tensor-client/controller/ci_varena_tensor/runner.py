"""Fresh stdlib-only launcher; the frozen client owns Torch/Varena installation."""

import hashlib
import importlib.metadata
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
assert case in ("typed_exports", "failed_tail")
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
script = "gpu.py" if case == "typed_exports" else "failed_tail_gpu.py"
# Invoke the frozen script's exact run() as its CLI does. No setup, warm-up,
# replaced operators, or frontend imports intervene before its own vt.install.
client = runpy.run_path(str(binaries / script))
if case == "typed_exports":
    value = client["run"](0, payload["delay_cycles"])
else:
    value = client["run"](0)
# Preserve the full original result before wrapper validation can fail.
print(json.dumps({"original_result": value}, sort_keys=True), flush=True)

import tensorfs
import torch
from tensorfs import _ext
from tensorfs.varena import api

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
if case == "typed_exports":
    expected_checks = [
        "typed interior export and independent override-stream producer ordering",
        "Use/Region close and trim preserve escaped and independent storage Pins",
        "interior record_stream and final-alias release wait actual pending consumers",
        "immutable host snapshot and half/float Source storage versions",
        "native dtype/layout bounds rejection and scalar/empty/broadcast exports",
    ]
    assert value["checks"] == expected_checks
    assert value["native_stats"]["weights_resident"] == 0
    assert value["device"] == 0 and value["delay_cycles"] == payload["delay_cycles"]
    scope = "five actual typed export checks; original stream retirement completed"
else:
    assert value["passed"] is True
    assert (
        value["scope"]
        == "intentional unproven storage quarantine; no reclamation claim"
    )
    assert value["retained_weight_bytes"] > 0
    assert value["after"]["quarantined"] == value["before"]["quarantined"] + 1
    assert (
        value["after"]["abandoned_cache_bytes"]
        == value["before"]["abandoned_cache_bytes"]
    )
    assert uuid_key(value["gpu_uuid"]) == uuid_key(payload["gpu_uuid"])
    scope = value["scope"]
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
            "distribution_root": str(root),
            "package_search_paths": list(tensorfs.__path__),
            "python_file_identity": python_files,
            "source_sha": payload["source_sha"],
            "script_source_sha": payload["script_source_sha"],
            "script_sha256": digest(binaries / script),
        },
        sort_keys=True,
    )
)
