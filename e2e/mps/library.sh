#!/usr/bin/env bash
# Actual checkpoint/alias/refill against the candidate libraries, before local CLI admission.
# No installed CLI/SDK, alternate home or daemon is created by this library-only phase.
. "$(dirname "$0")/../lib.sh"
parse "$@"
[ "$(uname -s)" = Darwin ] || fail "the MPS library gate runs on macOS"
[ -f "$CAND_TFS" ] && [ -f "$CAND_RT" ] || fail "current Mac wheels are required"
uv run --no-project --python 3.12 --with "$CAND_TFS" --with "$CAND_RT" --with 'torch>=2.14,<2.15' \
  python - "$E2E/../release-gate/suites/execution/mps-residency" "$OUT" "$CAND_COZY" "$CAND_TFS" "$CAND_RT" <<'PY'
import hashlib
import importlib.metadata
import json
import platform
import sys
from pathlib import Path

import torch
from cozy_runtime._build_provenance import COMMIT

sys.path.insert(0, sys.argv[1])
from ci_mps_residency import exercise

assert torch.backends.mps.is_available()
report = exercise(209103, torch.device("mps"))
assert report.passed
detail = json.loads(report.detail)
detail.update(runtime_commit=COMMIT, os=platform.platform(), architecture=platform.machine())
detail["artifacts"] = {}
for kind, value in zip(("cozy", "tensorfs", "runtime"), sys.argv[3:], strict=True):
    path = Path(value)
    with path.open("rb") as source:
        sha = hashlib.file_digest(source, "sha256").hexdigest()
    detail["artifacts"][kind] = {"filename": path.name, "sha256": sha, "bytes": path.stat().st_size}
detail["scope"] = "Real TensorFS/Torch MPS library checkpoint, alias lifetime and refill; not installed CLI inference"
Path(sys.argv[2], "mps-library.json").write_text(json.dumps(detail, indent=2) + "\n")
print(json.dumps(detail, indent=2))
PY
