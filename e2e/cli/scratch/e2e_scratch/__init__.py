"""shared_scratch's probe (cl-984): a temporary file, a file in the working directory and one under `~`, each holding the
call's marker, and the marker printed to stdout and stderr; then where those went. It waits `seconds` first, so another
call overlaps it, and `fail` raises after printing."""

import os
import sys
import tempfile
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    marker: str
    seconds: float = 0.0
    fail: bool = False


class Wrote(msgspec.Struct):
    pid: int
    temporary: str
    working: str
    home: str


@app.entrypoint(accelerator=False)
def probe(payload: Probe) -> Wrote:
    print(f"{payload.marker} stdout", flush=True)
    print(f"{payload.marker} stderr", file=sys.stderr, flush=True)
    with tempfile.NamedTemporaryFile("w", prefix="probe-", delete=False) as temporary:
        temporary.write(payload.marker)
    working = Path("probe.txt")
    working.write_text(payload.marker)
    home = Path("~/.cache/e2e-scratch").expanduser()
    home.mkdir(parents=True, exist_ok=True)
    (home / "probe.txt").write_text(payload.marker)
    time.sleep(payload.seconds)
    if payload.fail:
        raise RuntimeError(f"{payload.marker} failed as asked")
    return Wrote(os.getpid(), temporary.name, str(working.resolve()), str(home / "probe.txt"))
