# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""Install only the frozen diagnostic hook after unprofiled Comfy measurements."""
import hashlib
import json
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, FileAsset, Outputs

app = App()


class ProfileInstallRequest(msgspec.Struct):
    pass


class ProfileInstallResult(msgspec.Struct):
    receipt: str


B = Path('/var/tmp/bench4090-20261026')
PROFILE_SOURCE = None  # @PROFILE_SOURCE@
EXECUTION_SHA256 = '40c5306bc8d032aea90b93a7e1ef4ca137e0cf372187132d32b99ae070ba3d87'


@app.entrypoint(accelerator=False)
def main(payload: ProfileInstallRequest, ctx: Context, out: Outputs) -> ProfileInstallResult:
    assert isinstance(PROFILE_SOURCE, str) and PROFILE_SOURCE
    assert hashlib.sha256((B / 'ComfyUI/execution.py').read_bytes()).hexdigest() == EXECUTION_SHA256
    target = B / 'ComfyUI/custom_nodes/cozy_benchmark_profile/__init__.py'
    if target.exists():
        assert target.read_text() == PROFILE_SOURCE, 'different existing diagnostic hook'
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(PROFILE_SOURCE)
    record = {'path': str(target), 'sha256': hashlib.sha256(target.read_bytes()).hexdigest(),
              'execution_sha256': EXECUTION_SHA256, 'scope': 'diagnostic only; server restart required'}
    (B / 'profile-install.json').write_text(json.dumps(record, indent=2))
    return ProfileInstallResult(receipt=json.dumps(record))
