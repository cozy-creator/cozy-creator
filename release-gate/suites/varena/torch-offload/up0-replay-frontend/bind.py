"""Create a new immutable diagnostic bin from explicitly selected frozen artifacts."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import zipfile


SDK = {
    "original9d-c059": "4e70c84668529633753bc4f14531be2e69d297283ba601fc21ec6e8418909073",
    "controlled9d-guardb91-c059": "b91da06061f7a36add2d166595abd94ee513c3cbb4cc0f7b407e642f863154eb",
}
RUNTIME = "ca56fee0f92018d4bfc64f4b9c165f300781c3ed9afd805c4a43c7d4c7fd8f3d"
CLIENT = "e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652"
RETURNS = "7653cb76fe29e96f7a6d8d6ef81bf4030aa57623136ef41ebaf007f816964cfb"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def members(wheel, package):
    with zipfile.ZipFile(wheel) as archive:
        prefix = package + "/"
        return {name[len(prefix):]: hashlib.sha256(archive.read(name)).hexdigest()
                for name in archive.namelist()
                if name.startswith(prefix) and not name.endswith("/")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execution-base", choices=SDK, required=True)
    parser.add_argument("--sdk-wheel", type=Path, required=True)
    parser.add_argument("--runtime-wheel", type=Path, required=True)
    parser.add_argument("--frozen-bin", type=Path, required=True)
    parser.add_argument("--helper-handoff", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    assert sha(args.sdk_wheel) == SDK[args.execution_base]
    assert sha(args.runtime_wheel) == RUNTIME
    assert sha(args.frozen_bin / "original_sdxl.py") == CLIENT
    assert sha(args.frozen_bin / "capture_pressure_evidence.py") == RETURNS
    handoff = json.loads(args.helper_handoff.read_text())
    helper, header = Path(handoff["helper"]), Path(handoff["header"])
    assert sha(helper) == handoff["helper_sha256"]
    assert sha(header) == handoff["header_sha256"]
    assert handoff["format_version"] == 3
    assert "allocation_trace_workspace_cache_enabled" in header.read_text()
    expected_ids = set(range(100, 103)) | set(range(170, 173)) | set(range(200, 204))
    for start in (110, 120, 130, 140, 150, 160, 180, 190):
        expected_ids.update(range(start, start + 3))
    actual_ids = {int(value) for value in re.findall(r"XS138_\w+\s*=\s*(\d+)", header.read_text())}
    assert actual_ids == expected_ids
    source = Path(__file__).resolve().parent
    copies = {name: source / name for name in ("frontend_trace.py", "automatic_sdxl_gpu.py")}
    copies.update({"allocation_trace.so": helper, "diagnostic_markers.h": header,
                   "original_sdxl.py": args.frozen_bin / "original_sdxl.py",
                   "capture_pressure_evidence.py": args.frozen_bin / "capture_pressure_evidence.py"})
    # Refuse any existing destination. Frozen files/receipts are never overwritten.
    args.out.mkdir(parents=True, exist_ok=False)
    for name, origin in copies.items():
        shutil.copyfile(origin, args.out / name)
    manifest = {
        "trace_format_version": 3, "return_evidence_format_version": 2,
        "status": "prepared-unrun; actual CUDA719 run27056 remains red",
        "execution_base": args.execution_base,
        "sdk_source": "9d959e31d6fc58a2d4c1f7371bc3a2b6b658c6de",
        "controlled_patch": "9aec455c workspace guard; only if explicitly selected" if args.execution_base.startswith("controlled") else None,
        "runtime_source": "c059294c874183499ddadba5becc1d255a4e0763",
        "sdk_wheel_sha256": SDK[args.execution_base], "runtime_wheel_sha256": RUNTIME,
        "tensorfs_files": members(args.sdk_wheel, "tensorfs"),
        "runtime_files": members(args.runtime_wheel, "cozy_runtime"),
        "files": {name: sha(args.out / name) for name in copies},
        "development_context": json.loads((source / "ownership.json").read_text()),
        "helper_handoff_sha256": sha(args.helper_handoff),
        "settings": {"cap_bytes": 6 * 1024**3, "height": 1024, "width": 1024,
                     "steps": 20, "calls": 2, "seed": 1150504160,
                     "guidance": 5.0, "added_synchronization": False,
                     "added_body_retry": False, "launch_blocking": False},
    }
    (args.out / "frontend-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"output": str(args.out), "execution_base": args.execution_base,
                      "manifest_sha256": sha(args.out / "frontend-manifest.json"),
                      "status": "prepared-unrun"}, indent=2))


if __name__ == "__main__":
    main()
