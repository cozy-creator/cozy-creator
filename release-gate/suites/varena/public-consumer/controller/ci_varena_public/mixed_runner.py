"""Run the unchanged mixed GPU client with its actual selected import order."""

import hashlib
import json
import runpy
import sys
from pathlib import Path

binaries = Path(__file__).resolve().parent / "bin"
payload = json.loads(Path(sys.argv[1]).read_text())
order = sys.argv[2]
assert order in ("native-first", "python-first")
assert payload["gpu_uuid"] != "UNBOUND"
assert "torch" not in sys.modules and "tensorfs" not in sys.modules
manifest = json.loads((binaries / "manifest.json").read_text())
assert (
    hashlib.sha256((binaries / "mixed.py").read_bytes()).hexdigest()
    == manifest["python_helpers"]["mixed.py"]
)
sys.path.insert(0, str(binaries))
sys.argv = [
    str(binaries / "mixed.py"),
    "--c",
    str(binaries / "mixed-c.so"),
    "--rust",
    str(binaries / "mixed-rust.so"),
    "--order",
    order,
    "--gpu",
]
# This exact script owns all native/TensorFS imports and real mixed GPU operations.
# Do not pre-import either frontend merely to perform a provenance query.
runpy.run_path(sys.argv[0], run_name="__main__")

import torch  # Validate only after preserving the tested import order.
from tensorfs import _ext

uuid = str(torch.cuda.get_device_properties(0).uuid)


def key(value):
    return value.lower().removeprefix("gpu-").removeprefix("mig-").replace("-", "")


assert key(uuid) == key(payload["gpu_uuid"])
assert (
    hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest()
    == payload["extension_sha256"]
)
mapped = {
    line.split(maxsplit=5)[-1]
    for line in Path("/proc/self/maps").read_text().splitlines()
    if len(line.split(maxsplit=5)) == 6
}
for name, expected in (
    ("libvarena.so", payload["core_sha256"]),
    (payload["std_name"], payload["std_sha256"]),
):
    paths = {path for path in mapped if Path(path).name == name}
    assert (
        len(paths) == 1
        and hashlib.sha256(Path(next(iter(paths))).read_bytes()).hexdigest() == expected
    )
print(
    json.dumps(
        {
            "passed": True,
            "order": order,
            "gpu_uuid": uuid,
            "original_mixed_gpu_script_completed": True,
        }
    )
)
