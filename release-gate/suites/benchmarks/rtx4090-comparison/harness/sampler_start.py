# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""One rental's shared GPU/process sampler; no PCIe integration or H2D estimates.

Use the same session across both engines. An absolute deadline cannot be extended by
restarting the collector. /proc counters are cumulative per PID birth, not request
attribution; read_bytes is storage I/O, never a measurement of H2D transfer.
"""
from __future__ import annotations

import csv
import fcntl
import json
import math
import os
import re
import signal
import subprocess
import time
import uuid
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context

app = App()


class SamplerStartRequest(msgspec.Struct):
    session: str
    deadline_unix_s: float


class SamplerStartResult(msgspec.Struct):
    state: str


ROOT = Path("/var/tmp/cozy-4090-benchmark")
INTERVAL = 0.5
BASE_FIELDS = (
    "timestamp", "index", "uuid", "name", "memory.used", "memory.total",
    "utilization.gpu", "utilization.memory", "temperature.gpu", "clocks.current.graphics",
    "clocks.current.sm", "clocks.current.memory", "power.draw", "power.limit", "pstate",
)
THROTTLE_FIELDS = (
    "active", "gpu_idle", "applications_clocks_setting", "sw_power_cap", "hw_slowdown",
    "hw_thermal_slowdown", "hw_power_brake_slowdown", "sw_thermal_slowdown", "sync_boost",
)


def directory(session: str) -> Path:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,95}", session):
        raise ValueError("session must be a unique safe task identifier")
    return ROOT / session


def save(path: Path, value: object) -> None:
    temporary = path.with_name(path.name + "." + str(os.getpid()) + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n")
    temporary.replace(path)


def process(pid: int) -> dict[str, object]:
    path = Path("/proc") / str(pid)
    raw = (path / "stat").read_text()
    # comm can contain spaces and parentheses; fields after the last ')' start at3.
    fields = raw[raw.rfind(")") + 2:].split()
    return {
        "pid": pid, "start_ticks": int(fields[19]), "state": fields[0],
        "uid": path.stat().st_uid, "utime_ticks": int(fields[11]),
        "stime_ticks": int(fields[12]), "rss_bytes": int(fields[21]) * os.sysconf("SC_PAGE_SIZE"),
    }


def alive(owner: dict[str, object]) -> bool:
    try:
        now = process(int(owner["pid"]))
        return (owner["boot_id"] == Path("/proc/sys/kernel/random/boot_id").read_text().strip()
                and now["start_ticks"] == owner["start_ticks"] and now["uid"] == owner["uid"]
                and now["state"] != "Z")
    except (FileNotFoundError, ProcessLookupError):
        return False


def daemon(folder: Path, owner: dict[str, object], deadline: float) -> None:
    stopping = False

    def stop(signum: int, frame: object) -> None:
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    remaining = deadline - time.time()
    monotonic_end = time.monotonic() + max(0.0, remaining)
    boot = owner["boot_id"]
    errors: list[str] = []
    sample_count = 0
    current_child: subprocess.Popen[str] | None = None

    def finished() -> bool:
        if stopping or time.monotonic() >= monotonic_end or time.time() >= deadline:
            return True
        try:
            return json.loads((folder / "stop.json").read_text())["collector_id"] == owner["collector_id"]
        except (OSError, ValueError, KeyError):
            return False

    def query(arguments: list[str]) -> tuple[str, int]:
        nonlocal current_child
        before = time.monotonic_ns()
        current_child = subprocess.Popen(["nvidia-smi", *arguments], stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE, text=True)
        try:
            child = process(current_child.pid)
            owner["query_child"] = {**child, "boot_id": boot}
            save(folder / "owner.json", owner)
        except OSError:
            pass  # A completed child will be reaped by communicate below.
        try:
            timeout = min(2.0, max(0.01, monotonic_end - time.monotonic()))
            stdout, stderr = current_child.communicate(timeout=timeout)
            if current_child.returncode:
                raise RuntimeError(f"nvidia-smi exited{current_child.returncode}: {stderr.strip()[:500]}")
            return stdout, time.monotonic_ns() - before
        finally:
            if current_child.poll() is None:
                current_child.kill()
                current_child.wait()
            current_child = None
            owner.pop("query_child", None)
            save(folder / "owner.json", owner)

    try:
        help_text, _ = query(["--help-query-gpu"])
        # NVIDIA renamed throttle reasons to event reasons; preserve the actual queried names.
        prefix = "clocks_event_reasons" if "clocks_event_reasons.active" in help_text else "clocks_throttle_reasons"
        selected = [f"{prefix}.{field}" for field in THROTTLE_FIELDS if f"{prefix}.{field}" in help_text]
        if not selected:
            raise RuntimeError("nvidia-smi exposes no clock throttle/event reason fields")
        ids, _ = query(["--query-gpu=uuid", "--format=csv,noheader,nounits"])
        gpu_ids = [x.strip() for x in ids.splitlines() if x.strip()]
        if len(gpu_ids) != 1:
            raise RuntimeError(f"requires exactly one visible rental GPU, found{len(gpu_ids)}")
        gpu_id = gpu_ids[0]
        fields = [*BASE_FIELDS, *selected]
        save(folder / f"collector-{owner['collector_id']}.json", {
            **owner, "gpu_uuid": gpu_id, "query_fields": fields, "interval_seconds": INTERVAL,
            "deadline_unix_s": deadline, "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "process_scope": "GPU UUID filtered NVIDIA compute processes visible in this PID namespace",
            "io_scope": "/proc/PID/io cumulative process counters; read_bytes is storage reads, not H2D",
            "units": "nvidia-smi nounits: memory MiB; clocks MHz; temperature C; power W; utilization percent",
        })
        gpu_columns = ["unix_ns", "monotonic_ns", "collector_id", "sample", "query_wall_ns", *BASE_FIELDS, *THROTTLE_FIELDS, "error"]
        proc_columns = ["unix_ns", "monotonic_ns", "collector_id", "sample", "gpu_uuid", "query_wall_ns",
                        "pid", "start_ticks", "uid", "process_name", "gpu_memory_mib", "rss_bytes",
                        "utime_ticks", "stime_ticks", "read_bytes", "write_bytes", "rchar", "wchar", "error"]
        with (folder / "gpu.csv").open("a", buffering=1) as gpu_file, (folder / "processes.csv").open("a", buffering=1) as proc_file:
            gpu_writer = csv.DictWriter(gpu_file, gpu_columns)
            proc_writer = csv.DictWriter(proc_file, proc_columns)
            if gpu_file.tell() == 0:
                gpu_writer.writeheader()
            if proc_file.tell() == 0:
                proc_writer.writeheader()
            due = time.monotonic()
            while not finished():
                stamp = {"unix_ns": time.time_ns(), "monotonic_ns": time.monotonic_ns(),
                         "collector_id": owner["collector_id"], "sample": sample_count}
                gpu_good = False
                try:
                    text, wall_ns = query(["-i", gpu_id, "--query-gpu=" + ",".join(fields), "--format=csv,noheader,nounits"])
                    rows = list(csv.reader(text.splitlines()))
                    if len(rows) != 1 or len(rows[0]) != len(fields):
                        raise RuntimeError("unexpected GPU query shape")
                    values = dict(zip(fields, (x.strip() for x in rows[0]), strict=True))
                    values = {key.removeprefix(prefix + "."): value for key, value in values.items()}
                    if values["uuid"] != gpu_id:
                        raise RuntimeError("GPU identity changed")
                    gpu_writer.writerow({**stamp, "query_wall_ns": wall_ns, **values})
                    gpu_good = True
                except Exception as error:
                    gpu_writer.writerow({**stamp, "error": repr(error)})
                    errors.append(repr(error))
                if finished():
                    break
                try:
                    text, wall_ns = query(["-i", gpu_id, "--query-compute-apps=gpu_uuid,pid,process_name,used_gpu_memory", "--format=csv,noheader,nounits"])
                    entries = list(csv.reader(text.splitlines()))
                    if not entries:
                        proc_writer.writerow({**stamp, "gpu_uuid": gpu_id, "query_wall_ns": wall_ns,
                                              "error": "no visible compute processes"})
                    for entry in entries:
                        if len(entry) != 4 or entry[0].strip() != gpu_id:
                            raise RuntimeError("unexpected GPU process query shape")
                        _, raw_pid, name, memory = (x.strip() for x in entry)
                        row = {**stamp, "gpu_uuid": gpu_id, "query_wall_ns": wall_ns,
                               "pid": int(raw_pid), "process_name": name, "gpu_memory_mib": memory}
                        try:
                            first = process(int(raw_pid))
                            io = dict(line.split(":", 1) for line in (Path("/proc") / raw_pid / "io").read_text().splitlines())
                            last = process(int(raw_pid))
                            if (first["start_ticks"], first["uid"]) != (last["start_ticks"], last["uid"]):
                                raise RuntimeError("PID birth changed while sampling")
                            row.update({k: first[k] for k in ("start_ticks", "uid", "rss_bytes", "utime_ticks", "stime_ticks")})
                            row.update({k: int(io[k]) for k in ("read_bytes", "write_bytes", "rchar", "wchar")})
                        except (OSError, KeyError, ValueError, RuntimeError) as error:
                            row["error"] = repr(error)  # Host-PID/container-PID mismatch is missing data, never zero I/O.
                        proc_writer.writerow(row)
                except Exception as error:
                    proc_writer.writerow({**stamp, "gpu_uuid": gpu_id, "error": repr(error)})
                    errors.append(repr(error))
                sample_count += 1
                if gpu_good and not (folder / f"ready-{owner['collector_id']}.json").exists():
                    save(folder / f"ready-{owner['collector_id']}.json", {"gpu_uuid": gpu_id, "sample_count": sample_count})
                due += INTERVAL
                time.sleep(max(0.0, min(INTERVAL, due - time.monotonic(), monotonic_end - time.monotonic())))
    except BaseException as error:
        errors.append(repr(error))
    finally:
        if current_child is not None and current_child.poll() is None:
            current_child.kill()
            current_child.wait()
        save(folder / f"finished-{owner['collector_id']}.json", {
            "collector_id": owner["collector_id"], "unix_ns": time.time_ns(), "sample_count": sample_count,
            "errors": errors, "deadline_reached": time.time() >= deadline or time.monotonic() >= monotonic_end,
        })


@app.entrypoint(accelerator=False)
def main(payload: SamplerStartRequest, ctx: Context) -> SamplerStartResult:
    """Start once, or return the verified live collector; never extend the session deadline."""
    session, deadline_unix_s = payload.session, payload.deadline_unix_s
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("safe sampler cleanup requires Linux pidfd support")
    if not math.isfinite(deadline_unix_s) or not 0 < deadline_unix_s - time.time() <= 3600:
        raise ValueError("deadline_unix_s must be in the next3600seconds")
    folder = directory(session)
    folder.mkdir(parents=True, mode=0o700, exist_ok=True)
    with (folder / "start.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        session_path = folder / "session.json"
        if session_path.exists():
            saved = json.loads(session_path.read_text())
            if saved["deadline_unix_s"] != deadline_unix_s:
                raise ValueError("an existing session deadline cannot change")
        else:
            save(session_path, {"session": session, "deadline_unix_s": deadline_unix_s, "created_unix_ns": time.time_ns()})
        owner_path = folder / "owner.json"
        if owner_path.exists():
            owner = json.loads(owner_path.read_text())
            if alive(owner):
                return SamplerStartResult(state=json.dumps({"state": "already_running", **owner}, sort_keys=True))
        collector_id = uuid.uuid4().hex
        parent = os.fork()
        if parent == 0:
            os.close(lock.fileno())  # Do not hold the start/stop lock for the collector lifetime.
            os.setsid()
            if os.fork() == 0:
                for fd in (0, 1, 2):
                    with open(os.devnull, "r+b") as null:
                        os.dup2(null.fileno(), fd)
                # A detached sampler must not retain the executor control/spool descriptors.
                for name in os.listdir("/proc/self/fd"):
                    fd = int(name)
                    if fd > 2:
                        try:
                            os.close(fd)
                        except OSError:
                            pass
                owner = {**process(os.getpid()), "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                         "collector_id": collector_id, "session": session}
                save(owner_path, owner)
                with (folder / "owners.jsonl").open("a") as history:
                    history.write(json.dumps(owner, sort_keys=True) + "\n")
                try:
                    daemon(folder, owner, deadline_unix_s)
                finally:
                    os._exit(0)
            os._exit(0)
        os.waitpid(parent, 0)
        # A bounded readiness acknowledgement, not an inference timing delay.
        ready, ended = folder / f"ready-{collector_id}.json", folder / f"finished-{collector_id}.json"
        until = min(deadline_unix_s, time.time() + 8)
        while time.time() < until and not ready.exists() and not ended.exists():
            time.sleep(0.05)
        if not ready.exists():
            save(folder / "stop.json", {"collector_id": collector_id})
            detail = ended.read_text() if ended.exists() else "no readiness acknowledgement; stop requested"
            raise RuntimeError(detail)
        return SamplerStartResult(state=json.dumps({"state": "started", "directory": str(folder), **json.loads(owner_path.read_text())}, sort_keys=True))
