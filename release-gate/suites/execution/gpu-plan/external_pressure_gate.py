"""Qualify the real TensorD monitor with a finite, independently owned CUDA application.

Managed work and observations use the default CLI. The foreign application is a child of this
controller, not TensorD: a second Cozy job would correctly count as Cozy-owned GPU demand.
"""

import argparse
import json
import os
import queue
import subprocess
import threading
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate-python", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-foreign-mib", required=True, type=int)
    parser.add_argument("--expected-tensord", required=True)
    args = parser.parse_args()
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parent
    cozy = str(Path.home() / ".cozy/bin/cozy")
    python = str(args.candidate_python.absolute())  # preserve the isolated venv symlink
    events = queue.Queue()
    processes = {}
    receipt = {"began_ns": time.time_ns(), "controller_pid": os.getpid()}

    def cli(name, *arguments):
        result = subprocess.run(
            [cozy, "--json", *arguments],
            text=True,
            capture_output=True,
            timeout=120,
            check=False,
        )
        (output / f"{name}.ndjson").write_text(result.stdout)
        (output / f"{name}.stderr").write_text(result.stderr)
        result.check_returncode()
        return result.stdout

    machine = json.loads(cli("machine-before", "machine", "show", "local"))
    assert machine["state"] == "ready" and not machine["status"].get("runs"), machine
    assert machine["status"]["commit"] == args.expected_tensord, machine["status"]
    receipt["machine"] = {
        k: machine["status"][k] for k in ("commit", "runtime", "tensorfs")
    }

    def start(name, command):
        process = subprocess.Popen(
            command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
        )
        processes[name] = process

        def read():
            with (output / f"{name}.ndjson").open("w") as log:
                for line in process.stdout:
                    log.write(line)
                    log.flush()
                    try:
                        events.put((name, json.loads(line)))
                    except ValueError:
                        pass
            events.put((name, {"process_exit": process.wait()}))

        threading.Thread(target=read, daemon=True).start()
        return process

    run_id = None
    manual_cancel = False
    terminal = None
    try:
        start(
            "managed",
            [
                cozy,
                "--json",
                "run",
                str(root / "repeat"),
                f"candidate_python={python}",
                "mode=external_pressure",
                "--on",
                "local",
                "--gpus=1",
            ],
        )
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            try:
                name, row = events.get(timeout=1)
            except queue.Empty:
                continue
            if name == "foreign":
                if row.get("foreign_ready"):
                    receipt["foreign_ready"] = row
                elif row.get("foreign_result"):
                    receipt["foreign_result"] = row
                elif "process_exit" in row:
                    receipt["foreign_exit"] = row["process_exit"]
            else:
                if isinstance(row.get("run"), dict):
                    run_id = row["run"]["id"]
                    receipt["run_id"] = run_id
                    receipt["run_number"] = row["run"].get("number")
                    if row["run"]["status"] in ("completed", "failed", "canceled"):
                        terminal = row["run"]
                        receipt["terminal"] = terminal
                if row.get("type") == "run.log":
                    try:
                        detail = json.loads(row.get("text", ""))
                    except ValueError:
                        continue
                    if detail.get("pressure_ready") and "foreign" not in processes:
                        receipt["managed_ready"] = detail
                        pid = detail["pid"]
                        command = (
                            Path(f"/proc/{pid}/cmdline")
                            .read_bytes()
                            .replace(b"\0", b" ")
                            .decode()
                        )
                        assert python in command and "external_managed.py" in command, (
                            command
                        )
                        receipt["managed_birth"] = (
                            Path(f"/proc/{pid}/stat").read_text().split()[21]
                        )
                        # Let the existing 500ms monitor observe established managed demand.
                        time.sleep(1.1)
                        foreign = start(
                            "foreign",
                            [
                                python,
                                "-I",
                                str(root / "foreign_cuda.py"),
                                "--max-mib",
                                str(args.max_foreign_mib),
                            ],
                        )
                        receipt["foreign_pid"] = foreign.pid
                    elif detail.get("pressure_cleanup"):
                        receipt["managed_cleanup"] = detail
                if "process_exit" in row:
                    receipt["managed_exit"] = row["process_exit"]
            if terminal is not None and (
                "foreign" not in processes or "foreign_exit" in receipt
            ):
                break
        assert terminal is not None, "managed run did not settle"
        assert terminal["status"] == "canceled", terminal
        assert receipt.get("managed_cleanup", {}).get("native_bytes") == 0, receipt
        assert receipt.get("foreign_result", {}).get("passed"), receipt
        assert receipt.get("foreign_exit") == 0, receipt
        assert receipt["foreign_ready"]["parent_pid"] == os.getpid()
        pid = receipt["managed_ready"]["pid"]
        child = Path(f"/proc/{pid}/stat")
        assert (
            not child.exists()
            or child.read_text().split()[21] != receipt["managed_birth"]
        )
        time.sleep(
            1.1
        )  # observe the monitor releasing admission after the foreign context exits
        log = cli(
            "memory-after", "machine", "logs", "local", "memory", "--lines", "1000"
        )
        notes = []
        for line in log.splitlines():
            try:
                text = json.loads(line)["text"]
                stamp, payload = text.split(" ", 1)
                note = json.loads(payload)
                if float(stamp) * 1e9 >= receipt["began_ns"]:
                    notes.append({"at_ns": int(float(stamp) * 1e9), **note})
            except (ValueError, KeyError):
                continue
        receipt["monitor"] = [
            n for n in notes if n.get("event", "").startswith("gpu_pressure")
        ]
        canceled = next(
            n
            for n in notes
            if n.get("event") == "gpu_pressure_cancel" and n.get("root") == run_id
        )
        assert any(n.get("event") == "gpu_pressure" and n.get("blocked") for n in notes)
        assert any(
            n.get("event") == "gpu_pressure" and n.get("blocked") is False
            for n in notes
        )
        receipt["monitor_cancel_to_native_release_ms"] = (
            receipt["managed_cleanup"]["at_ns"] - canceled["at_ns"]
        ) / 1e6
        after = json.loads(cli("machine-after", "machine", "show", "local"))
        assert after["status"]["commit"] == args.expected_tensord, after["status"]
        receipt["passed"] = True
    finally:
        try:
            if run_id is not None and terminal is None:
                manual_cancel = True
                cli("cleanup-cancel", "run", "cancel", run_id)
            # Both owned programs have finite safety deadlines. Never kill unrelated work.
            for process in processes.values():
                process.wait(timeout=180)
        finally:
            receipt["manual_cleanup_cancel"] = manual_cancel
            (output / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(json.dumps(receipt, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
