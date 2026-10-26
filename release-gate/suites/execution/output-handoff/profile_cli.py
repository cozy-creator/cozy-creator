"""Timestamp an ordinary default-home Cozy command and its delivered outputs.

No SDK updates, daemon startup/stop, rentals, cancellation or retries are authored here.
The explicit command remains responsible for its own lifecycle. This observer does not
cancel a remote run on interruption. The primary wall clock includes process startup and
ends when the CLI returns; digest inspection happens outside that timed interval.
"""

import argparse
import datetime
import hashlib
import json
import signal
import subprocess
import sys
import threading
import time
from collections import deque
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--out", type=Path, required=True)
parser.add_argument("--expect-sha256")
parser.add_argument("args", nargs=argparse.REMAINDER)
args = parser.parse_args()
argv = args.args
if argv[:1] == ["--"]:
    argv = argv[1:]
if not argv:
    parser.error("pass an ordinary Cozy subcommand after --")
if "--json" not in argv:
    parser.error("the observed command must use --json")
cli = Path.home() / ".local/bin/cozy"
digest = hashlib.sha256(cli.read_bytes()).hexdigest()
if args.expect_sha256 and digest != args.expect_sha256:
    raise SystemExit(
        "Caller differs from explicitly expected artifact; command not started"
    )
out = args.out.resolve()
out.mkdir(parents=True, exist_ok=False)
clock_origin = time.monotonic_ns()
epoch_origin = time.time_ns()
metadata = {
    "argv": [str(cli), *argv],
    "caller_sha256_before": digest,
    "started_utc": datetime.datetime.now(datetime.UTC).isoformat(),
    "clock_origin_ns": clock_origin,
    "epoch_origin_ns": epoch_origin,
    "scope": "CLI invocation to process completion; outputs checked afterward; event arrivals are local observations, not remote stage timestamps.",
}
(out / "command.json").write_text(json.dumps(metadata, indent=2) + "\n")
events = deque(maxlen=10000)
event_count = 0
errors = []


class Sink:
    """A failed journal never prevents draining the owned CLI's pipe."""

    def __init__(self, name):
        self.name = name
        try:
            self.file = (out / name).open("w")
        except OSError as error:
            errors.append(name + ": " + repr(error))
            self.file = None

    def write(self, text):
        if self.file is None:
            return
        try:
            self.file.write(text)
            self.file.flush()
        except OSError as error:
            errors.append(self.name + ": " + repr(error))
            self.close()

    def close(self):
        file, self.file = self.file, None
        if file is not None:
            try:
                file.close()
            except OSError as error:
                errors.append(self.name + " close: " + repr(error))


def detach_on_reader_failure(label, error):
    errors.append(label + ": " + repr(error))
    # Stop only this CLI observer, never the remote run or a shared service.
    if child.poll() is None:
        try:
            child.send_signal(signal.SIGINT)
        except ProcessLookupError:
            pass


def pump_stdout(stream):
    global event_count
    pending = ""
    decoder = json.JSONDecoder()
    raw = Sink("stdout.json")
    journal = Sink("event-arrivals.jsonl")
    try:
        for line in iter(stream.readline, ""):
            observed = time.monotonic_ns()
            raw.write(line)
            pending += line
            while pending.strip():
                pending = pending.lstrip()
                try:
                    value, end = decoder.raw_decode(pending)
                except ValueError:
                    break
                event = {
                    "received_elapsed_ns": observed - clock_origin,
                    "received_epoch_ns": epoch_origin + observed - clock_origin,
                    "value": value,
                }
                events.append(event)
                event_count += 1
                journal.write(json.dumps(event, separators=(",", ":")) + "\n")
                pending = pending[end:]
        if pending.strip():
            errors.append("Trailing stdout was not complete JSON; raw output preserved")
    except Exception as error:  # noqa: BLE001 - detach the owned observer on reader failure
        detach_on_reader_failure("stdout observer", error)
    finally:
        raw.close()
        journal.close()
        stream.close()


def pump_stderr(stream):
    raw = Sink("stderr.txt")
    journal = Sink("stderr-arrivals.jsonl")
    try:
        for line in iter(stream.readline, ""):
            observed = time.monotonic_ns()
            raw.write(line)
            row = {
                "received_elapsed_ns": observed - clock_origin,
                "line": line.rstrip("\n"),
            }
            journal.write(json.dumps(row, separators=(",", ":")) + "\n")
    except Exception as error:  # noqa: BLE001 - detach the owned observer on reader failure
        detach_on_reader_failure("stderr observer", error)
    finally:
        raw.close()
        journal.close()
        stream.close()


clock_origin = time.monotonic_ns()
epoch_origin = time.time_ns()
metadata.update(clock_origin_ns=clock_origin, epoch_origin_ns=epoch_origin)
child = subprocess.Popen(
    metadata["argv"],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
    start_new_session=True,
)
metadata["pid"] = child.pid
try:
    (out / "process.json").write_text(json.dumps(metadata, indent=2) + "\n")
except OSError as error:
    errors.append("process.json: " + repr(error))
threads = [
    threading.Thread(target=pump_stdout, args=(child.stdout,), daemon=True),
    threading.Thread(target=pump_stderr, args=(child.stderr,), daemon=True),
]
for thread in threads:
    thread.start()
interrupted = False
try:
    code = child.wait()
except KeyboardInterrupt:
    # This CLI's documented SIGINT behavior only detaches its observer. Never cancel a run.
    interrupted = True
    if child.poll() is None:
        child.send_signal(signal.SIGINT)
    code = child.wait()
ended = time.monotonic_ns()
for thread in threads:
    thread.join()
run = None
paths = {}
timeline = []
for event in events:
    value = event["value"]
    if not isinstance(value, dict):
        continue
    if isinstance(value.get("run"), dict):
        run = value["run"]
    elif "number" in value and "status" in value:
        run = value
    kind = value.get("type", "snapshot")
    timeline.append(
        {
            "event": kind,
            "received_elapsed_ms": event["received_elapsed_ns"] / 1e6,
            "sequence_number": value.get("sequence_number"),
            "remote_at_ms": value.get("at_ms"),
        }
    )
    item = value.get("item", {})
    if item.get("path"):
        paths[item["path"]] = item
    for item in (
        value.get("output", [])
        if kind == "snapshot"
        else value.get("run", {}).get("output", [])
    ):
        if item.get("path"):
            paths[item["path"]] = item
outputs = []
for name, item in paths.items():
    path = Path(name)
    row = {
        "path": name,
        "exists": path.is_file(),
        "expected_digest": item.get("sha256"),
    }
    try:
        if not path.is_file():
            errors.append("Delivered output missing: " + name)
        else:
            stat = path.stat()
            row.update(bytes=stat.st_size, mtime_ns=stat.st_mtime_ns)
            with path.open("rb") as stream:
                row["sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
            want = item.get("sha256", "")
            if want.startswith("sha256:"):
                row["hash_matches"] = want[7:] == row["sha256"]
                if not row["hash_matches"]:
                    errors.append("Delivered output digest mismatch: " + name)
            if item.get("length") is not None and stat.st_size != item["length"]:
                errors.append("Delivered output length mismatch: " + name)
    except OSError as error:
        row["verification_error"] = repr(error)
        errors.append("Delivered output verification: " + repr(error))
    outputs.append(row)

result = {
    **metadata,
    "returncode": code,
    "interrupted_observer": interrupted,
    "full_round_trip_s": (ended - clock_origin) / 1e9,
    "caller_sha256_after": hashlib.sha256(cli.read_bytes()).hexdigest(),
    "timeline": timeline,
    "run": run,
    "outputs": outputs,
    "observer_errors": errors,
    "events_received": event_count,
    "events_retained_in_memory": len(events),
    "scope_note": "Raw journals retain all events unless a recorded write error occurred; receipt summaries retain only the last10000.",
}
(out / "receipt.json").write_text(json.dumps(result, indent=2) + "\n")
print(
    json.dumps(
        {
            "returncode": code,
            "full_round_trip_s": result["full_round_trip_s"],
            "events": event_count,
            "outputs": len(outputs),
            "observer_errors": errors,
            "events_received": event_count,
            "events_retained_in_memory": len(events),
            "scope_note": "Raw journals retain all events unless a recorded write error occurred; receipt summaries retain only the last10000.",
        }
    )
)
sys.exit(code if not errors else 1)
