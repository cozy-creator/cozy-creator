# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""Stop only this session's verified sampler birth; return GPU/process CSV and metadata."""
import fcntl
import json
import os
import re
import select
import signal
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, FileAsset, Outputs

app = App()


class SamplerStopRequest(msgspec.Struct):
    session: str


class SamplerStopResult(msgspec.Struct):
    stop: str
    files: list[FileAsset]


ROOT = Path("/var/tmp/cozy-4090-benchmark")


def identity(pid: int) -> dict[str, object]:
    path = Path("/proc") / str(pid)
    raw = (path / "stat").read_text()
    fields = raw[raw.rfind(")") + 2:].split()
    return {"start_ticks": int(fields[19]), "uid": path.stat().st_uid, "state": fields[0]}


def owned_pidfd(owner: dict[str, object]) -> int | None:
    """pidfd binds the process birth, eliminating the PID-reuse signal race."""
    if owner["boot_id"] != Path("/proc/sys/kernel/random/boot_id").read_text().strip():
        return None
    try:
        fd = os.pidfd_open(int(owner["pid"]))
    except ProcessLookupError:
        return None
    try:
        seen = identity(int(owner["pid"]))
        if seen["start_ticks"] != owner["start_ticks"] or seen["uid"] != owner["uid"] or seen["state"] == "Z":
            os.close(fd)
            return None
        return fd
    except (FileNotFoundError, ProcessLookupError):
        os.close(fd)
        return None
    except OSError:
        os.close(fd)
        raise


def ended(fd: int, seconds: float) -> bool:
    return bool(select.select([fd], [], [], seconds)[0])


def signal_owned(owner: dict[str, object], sig: signal.Signals) -> None:
    fd = owned_pidfd(owner)
    if fd is not None:
        try:
            signal.pidfd_send_signal(fd, sig)
            if not ended(fd, 3):
                raise RuntimeError("owned query child did not exit; cleanup is incomplete")
        except ProcessLookupError:
            pass
        finally:
            os.close(fd)


@app.entrypoint(accelerator=False)
def main(payload: SamplerStopRequest, ctx: Context, out: Outputs) -> SamplerStopResult:
    session = payload.session
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,95}", session):
        raise ValueError("session must be the sampler's safe task identifier")
    folder = ROOT / session
    with (folder / "start.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        owner = json.loads((folder / "owner.json").read_text())
        if owner["session"] != session:
            raise RuntimeError("sampler session identity mismatch")
        stop = folder / "stop.json"
        temporary = folder / ("stop." + str(os.getpid()) + ".tmp")
        temporary.write_text(json.dumps({"collector_id": owner["collector_id"]}) + "\n")
        temporary.replace(stop)
        fd = owned_pidfd(owner)
        forced = False
        try:
            if fd is not None and not ended(fd, 5):
                signal.pidfd_send_signal(fd, signal.SIGTERM)
                if not ended(fd, 3):
                    # A query child belongs only to the same recorded collector generation.
                    current = json.loads((folder / "owner.json").read_text())
                    if current["collector_id"] != owner["collector_id"]:
                        raise RuntimeError("collector ownership changed during stop")
                    if current.get("query_child"):
                        signal_owned(current["query_child"], signal.SIGKILL)
                    signal.pidfd_send_signal(fd, signal.SIGKILL)
                    forced = True
                    if not ended(fd, 3):
                        raise RuntimeError("owned sampler did not exit; do not claim cleanup")
        except ProcessLookupError:
            pass
        finally:
            if fd is not None:
                os.close(fd)
        # Also reap a recorded query child if its collector died before its finally block.
        current = json.loads((folder / "owner.json").read_text())
        if current["collector_id"] != owner["collector_id"]:
            raise RuntimeError("collector ownership changed during stop")
        if current.get("query_child"):
            signal_owned(current["query_child"], signal.SIGKILL)
        stop_record = {"collector_id": owner["collector_id"], "stopped_unix_ns": time.time_ns(),
                       "verified_birth_gone": True, "forced": forced,
                       "gpu_csv": (folder / "gpu.csv").is_file(),
                       "processes_csv": (folder / "processes.csv").is_file()}
        receipt = folder / f"stop-{owner['collector_id']}.json"
        receipt.write_text(json.dumps(stop_record, sort_keys=True) + "\n")
        paths = [folder / name for name in ("gpu.csv", "processes.csv", "session.json", "owners.jsonl", "owner.json")]
        paths.extend(sorted(folder.glob("collector-*.json")))
        paths.extend(sorted(folder.glob("finished-*.json")))
        paths.extend(sorted(folder.glob("stop-*.json")))
        # The stop receipt (incl. csv presence + verified_birth_gone) rides run['result']['stop'];
        # the csv/json files are binary outputs matched by media type.
        files = [out.save_file(path, media_type="text/csv" if path.suffix == ".csv" else "application/json")
                 for path in paths if path.is_file()]
        return SamplerStopResult(stop=json.dumps(stop_record), files=files)
